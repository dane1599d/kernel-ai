//! Plans one object's place in the session and writes it as a module.
//!
//! The module's function index space is: allowed host imports, then the object's
//! own functions in order, then one thunk per function an earlier entry defines.

use crate::object::{Object, Patchable};
use crate::{
    Allow, Def, ENV, Error, FD_WRITE, FuncType, KAI, Linked, MEMORY, STACK_POINTER, Session,
    Symbol, TABLE, ValType, WASI, leb,
};
use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use wasm_encoder as enc;
use wasmparser::{
    BinaryReader, DataSectionReader, RelocationEntry, RelocationType, SymbolFlags, SymbolInfo,
};

/// Most alignment a data segment may ask for: 64 KiB.
const MAX_ALIGN_LOG2: u32 = 16;

/// Where an imported function of the object goes.
#[derive(Clone, Copy)]
enum Callee {
    /// The n-th host import of the module.
    Host(u32),
    /// The n-th thunk.
    Thunk(u32),
}

/// What a symbol resolved to.
#[derive(Clone, Copy)]
enum Resolved {
    /// A function, by the object's function index.
    Func(u32),
    Data(u32),
    StackPointer,
    Table,
    Section,
}

struct Plan<'a> {
    host: Vec<(&'a str, &'a str, u32)>,
    callees: Vec<Callee>,
    thunks: Vec<(u32, u32)>,
    /// Table slot of each of the object's functions, imports included.
    slots: Vec<Option<u32>>,
    next_slot: u32,
    segment_addr: Vec<u32>,
    data_end: u32,
    resolved: Vec<Resolved>,
    defs: Vec<Def>,
    exports: Vec<(String, u32)>,
    retyped: Vec<String>,
}

pub(crate) fn link(session: &Session, obj: &Object, allow: Allow) -> Result<Linked, Error> {
    let mut plan = Plan {
        host: Vec::new(),
        callees: Vec::new(),
        thunks: Vec::new(),
        slots: vec![None; obj.func_imports.len() + obj.funcs.len()],
        next_slot: session.next_slot,
        segment_addr: Vec::new(),
        data_end: session.data_end,
        resolved: Vec::new(),
        defs: Vec::new(),
        exports: Vec::new(),
        retyped: Vec::new(),
    };
    plan.lay_out_data(session, obj)?;
    plan.resolve_imports(session, obj, allow)?;
    plan.resolve_symbols(session, obj)?;
    for section in [&obj.code, &obj.data].into_iter().flatten() {
        plan.slot_address_taken(session, obj, &section.relocs)?;
    }
    let code = obj.code.as_ref().map(|s| plan.patch(obj, s)).transpose()?;
    let data = obj.data.as_ref().map(|s| plan.patch(obj, s)).transpose()?;
    let wasm = plan.encode(obj, code.as_deref(), data.as_deref())?;
    Ok(Linked {
        wasm,
        table_size: plan.next_slot,
        data_end: plan.data_end,
        defs: plan.defs,
        exports: plan.exports.into_iter().map(|(name, _)| name).collect(),
        retyped: plan.retyped,
    })
}

fn align_up(value: u32, log2: u32) -> Option<u32> {
    let mask = (1u32 << log2) - 1;
    value.checked_add(mask).map(|v| v & !mask)
}

fn segment_sizes(data: Option<&Patchable>) -> Result<Vec<u32>, Error> {
    let Some(data) = data else {
        return Ok(Vec::new());
    };
    let reader = DataSectionReader::new(BinaryReader::new(data.payload, 0))
        .map_err(|e| Error::Parse(e.message().to_string()))?;
    let mut sizes = Vec::new();
    for segment in reader {
        let segment = segment.map_err(|e| Error::Parse(e.message().to_string()))?;
        sizes.push(segment.data.len() as u32);
    }
    Ok(sizes)
}

impl<'a> Plan<'a> {
    fn lay_out_data(&mut self, session: &Session, obj: &Object) -> Result<(), Error> {
        let sizes = segment_sizes(obj.data.as_ref())?;
        let mut cursor = self.data_end;
        for (size, &align) in sizes.iter().zip(&obj.segment_align) {
            if align > MAX_ALIGN_LOG2 {
                return Err(Error::Unsupported(format!("alignment 2^{align}")));
            }
            let addr = align_up(cursor, align).ok_or(Error::Full("memory"))?;
            cursor = addr.checked_add(*size).ok_or(Error::Full("memory"))?;
            if cursor > session.layout.memory_limit {
                return Err(Error::Full("memory"));
            }
            self.segment_addr.push(addr);
        }
        self.data_end = cursor;
        Ok(())
    }

    /// The symbol name of each imported function, from the symbol table.
    fn import_names(obj: &'a Object) -> BTreeMap<u32, &'a str> {
        let mut names = BTreeMap::new();
        for symbol in &obj.symbols {
            if let SymbolInfo::Func {
                flags,
                index,
                name: Some(name),
            } = symbol
                && flags.contains(SymbolFlags::UNDEFINED)
            {
                names.insert(*index, *name);
            }
        }
        names
    }

    fn resolve_imports(
        &mut self,
        session: &Session,
        obj: &'a Object,
        allow: Allow,
    ) -> Result<(), Error> {
        let names = Self::import_names(obj);
        for (index, import) in obj.func_imports.iter().enumerate() {
            let ty = obj.ty(import.ty)?;
            let host = match import.module {
                KAI => allow(import.field, ty),
                WASI => import.field == FD_WRITE && *ty == fd_write_type(),
                _ => false,
            };
            if host {
                let at = self.host.len() as u32;
                self.host.push((import.module, import.field, import.ty));
                self.callees.push(Callee::Host(at));
                continue;
            }
            if import.module == KAI || import.module == WASI {
                return Err(Error::Import {
                    module: import.module.to_string(),
                    field: import.field.to_string(),
                });
            }
            let name = names.get(&(index as u32)).copied().unwrap_or(import.field);
            match session.symbol(name) {
                Some(Symbol::Func { slot, ty: defined }) if defined == ty => {
                    self.slots[index] = Some(*slot);
                    self.callees.push(Callee::Thunk(self.thunks.len() as u32));
                    self.thunks.push((import.ty, *slot));
                }
                Some(_) => return Err(Error::TypeMismatch(name.to_string())),
                None => return Err(Error::Undefined(name.to_string())),
            }
        }
        Ok(())
    }

    fn new_slot(&mut self, session: &Session) -> Result<u32, Error> {
        if self.next_slot >= session.layout.table_limit {
            return Err(Error::Full("table"));
        }
        self.next_slot += 1;
        Ok(self.next_slot - 1)
    }

    fn resolve_symbols(&mut self, session: &Session, obj: &Object) -> Result<(), Error> {
        let imports = obj.func_imports.len() as u32;
        let total = imports + obj.funcs.len() as u32;
        for symbol in &obj.symbols {
            let resolved = match *symbol {
                SymbolInfo::Func { flags, index, name } => {
                    if index >= total || flags.contains(SymbolFlags::UNDEFINED) != (index < imports)
                    {
                        return Err(Error::Parse(format!("function symbol {index}")));
                    }
                    let global = !flags.contains(SymbolFlags::BINDING_LOCAL);
                    if index >= imports && global {
                        let name = name.ok_or_else(|| Error::Parse("unnamed function".into()))?;
                        self.define_func(session, obj, name, index)?;
                    }
                    Resolved::Func(index)
                }
                SymbolInfo::Data {
                    flags,
                    name,
                    symbol,
                } => Resolved::Data(self.resolve_data(session, flags, name, symbol)?),
                SymbolInfo::Global { flags, index, name } => {
                    let import = obj.global_imports.get(index as usize);
                    let field = name.or(import.map(|i| i.field));
                    match (import, field) {
                        (Some(i), Some(STACK_POINTER))
                            if flags.contains(SymbolFlags::UNDEFINED) && i.module == ENV =>
                        {
                            Resolved::StackPointer
                        }
                        _ => {
                            return Err(Error::Unsupported(format!(
                                "global {}",
                                field.unwrap_or("?")
                            )));
                        }
                    }
                }
                SymbolInfo::Table { .. } => Resolved::Table,
                SymbolInfo::Section { .. } => Resolved::Section,
                SymbolInfo::Event { .. } => {
                    return Err(Error::Unsupported("exceptions (setjmp)".into()));
                }
            };
            self.resolved.push(resolved);
        }
        Ok(())
    }

    fn define_func(
        &mut self,
        session: &Session,
        obj: &Object,
        name: &str,
        index: u32,
    ) -> Result<(), Error> {
        if self.defs.iter().any(|d| d.name == name) {
            return Err(Error::Parse(format!("{name} defined twice")));
        }
        let def_index = (index as usize) - obj.func_imports.len();
        let ty = obj.ty(obj.funcs[def_index])?.clone();
        let (slot, replaces) = match session.symbol(name) {
            Some(Symbol::Func { slot, ty: old }) if *old == ty => (*slot, true),
            Some(Symbol::Func { .. }) => {
                self.retyped.push(name.to_string());
                (self.new_slot(session)?, true)
            }
            Some(Symbol::Data { .. }) => return Err(Error::TypeMismatch(name.to_string())),
            None => (self.new_slot(session)?, false),
        };
        self.slots[index as usize] = Some(slot);
        self.exports.push((name.to_string(), index));
        self.defs.push(Def {
            name: name.to_string(),
            symbol: Symbol::Func { slot, ty },
            replaces,
        });
        Ok(())
    }

    fn resolve_data(
        &mut self,
        session: &Session,
        flags: SymbolFlags,
        name: &str,
        symbol: Option<wasmparser::DefinedDataSymbol>,
    ) -> Result<u32, Error> {
        let Some(defined) = symbol else {
            return match session.symbol(name) {
                Some(Symbol::Data { addr, .. }) => Ok(*addr),
                Some(_) => Err(Error::TypeMismatch(name.to_string())),
                None => Err(Error::Undefined(name.to_string())),
            };
        };
        let base = *self
            .segment_addr
            .get(defined.index as usize)
            .ok_or_else(|| Error::Parse(format!("data symbol {name}: no segment")))?;
        let addr = base
            .checked_add(defined.offset)
            .filter(|end| {
                end.checked_add(defined.size)
                    .is_some_and(|e| e <= self.data_end)
            })
            .ok_or_else(|| Error::Parse(format!("data symbol {name} outside its segment")))?;
        if !flags.contains(SymbolFlags::BINDING_LOCAL) {
            let replaces = match session.symbol(name) {
                Some(Symbol::Func { .. }) => return Err(Error::TypeMismatch(name.to_string())),
                found => found.is_some(),
            };
            if self.defs.iter().any(|d| d.name == name) {
                return Err(Error::Parse(format!("{name} defined twice")));
            }
            self.defs.push(Def {
                name: name.to_string(),
                symbol: Symbol::Data {
                    addr,
                    size: defined.size,
                },
                replaces,
            });
        }
        Ok(addr)
    }

    /// Gives a slot to each static function whose address is taken.
    fn slot_address_taken(
        &mut self,
        session: &Session,
        obj: &Object,
        relocs: &[RelocationEntry],
    ) -> Result<(), Error> {
        for reloc in relocs {
            if !matches!(
                reloc.ty,
                RelocationType::TableIndexSleb | RelocationType::TableIndexI32
            ) {
                continue;
            }
            let Some(Resolved::Func(index)) = self.resolved.get(reloc.index as usize).copied()
            else {
                return Err(Error::Parse("table index of a non-function".into()));
            };
            if self.slots[index as usize].is_none() {
                debug_assert!(index as usize >= obj.func_imports.len());
                self.slots[index as usize] = Some(self.new_slot(session)?);
            }
        }
        Ok(())
    }

    /// The module function index of the object's function `index`.
    fn func_index(&self, obj: &Object, index: u32) -> u32 {
        let host = self.host.len() as u32;
        let imports = obj.func_imports.len() as u32;
        if index >= imports {
            return host + index - imports;
        }
        match self.callees[index as usize] {
            Callee::Host(at) => at,
            Callee::Thunk(t) => host + obj.funcs.len() as u32 + t,
        }
    }

    fn symbol(&self, index: u32) -> Result<Resolved, Error> {
        self.resolved
            .get(index as usize)
            .copied()
            .ok_or_else(|| Error::Parse(format!("symbol {index} out of range")))
    }

    /// A copy of `section`'s payload with every relocation applied.
    fn patch(&self, obj: &Object, section: &Patchable) -> Result<Vec<u8>, Error> {
        let mut out = section.payload.to_vec();
        for reloc in &section.relocs {
            let range = reloc.relocation_range();
            let field = out
                .get_mut(range)
                .ok_or_else(|| Error::Parse("relocation outside its section".into()))?;
            let resolved = match reloc.ty {
                RelocationType::TypeIndexLeb => None,
                _ => Some(self.symbol(reloc.index)?),
            };
            let address = |addr: u32| {
                i64::from(addr)
                    .checked_add(reloc.addend)
                    .and_then(|a| u32::try_from(a).ok())
                    .ok_or_else(|| Error::Parse("address with addend out of range".into()))
            };
            match (reloc.ty, resolved) {
                (RelocationType::FunctionIndexLeb, Some(Resolved::Func(i))) => {
                    leb::put_u32_padded(field, self.func_index(obj, i));
                }
                (RelocationType::TableIndexSleb, Some(Resolved::Func(i))) => {
                    leb::put_i32_padded(field, self.slots[i as usize].unwrap_or(0) as i32);
                }
                (RelocationType::TableIndexI32, Some(Resolved::Func(i))) => {
                    field.copy_from_slice(&self.slots[i as usize].unwrap_or(0).to_le_bytes());
                }
                (RelocationType::MemoryAddrLeb, Some(Resolved::Data(a))) => {
                    leb::put_u32_padded(field, address(a)?);
                }
                (RelocationType::MemoryAddrSleb, Some(Resolved::Data(a))) => {
                    leb::put_i32_padded(field, address(a)? as i32);
                }
                (RelocationType::MemoryAddrI32, Some(Resolved::Data(a))) => {
                    field.copy_from_slice(&address(a)?.to_le_bytes());
                }
                (RelocationType::TypeIndexLeb, None) => {
                    obj.ty(reloc.index)?;
                    leb::put_u32_padded(field, reloc.index);
                }
                (RelocationType::GlobalIndexLeb, Some(Resolved::StackPointer))
                | (RelocationType::TableNumberLeb, Some(Resolved::Table)) => {
                    leb::put_u32_padded(field, 0);
                }
                (ty, _) => {
                    return Err(Error::Unsupported(format!("relocation {ty:?} here")));
                }
            }
        }
        Ok(out)
    }

    fn encode(
        &self,
        obj: &Object,
        code: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Vec<u8>, Error> {
        let mut module = enc::Module::new();
        let mut types = enc::TypeSection::new();
        for ty in &obj.types {
            types.ty().function(
                ty.params.iter().map(|v| val(*v)),
                ty.results.iter().map(|v| val(*v)),
            );
        }
        module.section(&types);
        module.section(&self.imports());
        let mut funcs = enc::FunctionSection::new();
        for ty in obj.funcs.iter().chain(self.thunks.iter().map(|(ty, _)| ty)) {
            funcs.function(*ty);
        }
        module.section(&funcs);
        let mut exports = enc::ExportSection::new();
        for (name, index) in &self.exports {
            exports.export(name, enc::ExportKind::Func, self.func_index(obj, *index));
        }
        module.section(&exports);
        module.section(&self.elements(obj));
        module.section(&self.code(obj, code)?);
        if let Some(data) = data {
            module.section(&self.data(data)?);
        }
        Ok(module.finish())
    }

    fn imports(&self) -> enc::ImportSection {
        let mut imports = enc::ImportSection::new();
        imports.import(
            ENV,
            MEMORY,
            enc::MemoryType {
                minimum: 0,
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            },
        );
        imports.import(
            ENV,
            TABLE,
            enc::TableType {
                element_type: enc::RefType::FUNCREF,
                table64: false,
                minimum: 0,
                maximum: None,
                shared: false,
            },
        );
        imports.import(
            ENV,
            STACK_POINTER,
            enc::GlobalType {
                val_type: enc::ValType::I32,
                mutable: true,
                shared: false,
            },
        );
        for (module, field, ty) in &self.host {
            imports.import(module, field, enc::EntityType::Function(*ty));
        }
        imports
    }

    /// One active element per function with a slot of its own.
    fn elements(&self, obj: &Object) -> enc::ElementSection {
        let mut elements = enc::ElementSection::new();
        let imports = obj.func_imports.len();
        for (index, slot) in self.slots.iter().enumerate().skip(imports) {
            if let Some(slot) = slot {
                let funcs = [self.func_index(obj, index as u32)];
                elements.active(
                    None,
                    &enc::ConstExpr::i32_const(*slot as i32),
                    enc::Elements::Functions(Cow::Borrowed(&funcs)),
                );
            }
        }
        elements
    }

    fn code(&self, obj: &Object, code: Option<&[u8]>) -> Result<enc::CodeSection, Error> {
        let mut section = enc::CodeSection::new();
        let code = code.unwrap_or(&[0]);
        let mut at = 0;
        let count = leb::read_u32(code, &mut at)?;
        if count as usize != obj.funcs.len() {
            return Err(Error::Parse("code and function counts differ".into()));
        }
        for _ in 0..count {
            let size = leb::read_u32(code, &mut at)? as usize;
            let body = at
                .checked_add(size)
                .and_then(|end| code.get(at..end))
                .ok_or_else(|| Error::Parse("function body past the section".into()))?;
            section.raw(body);
            at += size;
        }
        if at != code.len() {
            return Err(Error::Parse("bytes after the last function body".into()));
        }
        for (ty, slot) in &self.thunks {
            section.function(&thunk(obj.ty(*ty)?, *ty, *slot));
        }
        Ok(section)
    }

    fn data(&self, data: &[u8]) -> Result<enc::DataSection, Error> {
        let reader = DataSectionReader::new(BinaryReader::new(data, 0))
            .map_err(|e| Error::Parse(e.message().to_string()))?;
        let mut section = enc::DataSection::new();
        for (segment, addr) in reader.into_iter().zip(&self.segment_addr) {
            let segment = segment.map_err(|e| Error::Parse(e.message().to_string()))?;
            let offset = enc::ConstExpr::i32_const(*addr as i32);
            section.active(0, &offset, segment.data.iter().copied());
        }
        Ok(section)
    }
}

/// A function that passes its arguments to the function in `slot`.
fn thunk(ty: &FuncType, type_index: u32, slot: u32) -> enc::Function {
    let mut function = enc::Function::new([]);
    for local in 0..ty.params.len() as u32 {
        function.instructions().local_get(local);
    }
    function
        .instructions()
        .i32_const(slot as i32)
        .call_indirect(0, type_index)
        .end();
    function
}

fn val(ty: ValType) -> enc::ValType {
    match ty {
        ValType::I32 => enc::ValType::I32,
        ValType::I64 => enc::ValType::I64,
        ValType::F32 => enc::ValType::F32,
        ValType::F64 => enc::ValType::F64,
    }
}

/// `fd_write(fd, iovs, iovs_len, nwritten) -> errno`.
pub(crate) fn fd_write_type() -> FuncType {
    FuncType {
        params: vec![ValType::I32; 4],
        results: vec![ValType::I32],
    }
}
