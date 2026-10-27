//! Reads a relocatable object (Linking.md) into the parts the linker needs. Only
//! what `wcc -c` writes is accepted: memory, table and globals are imported, and
//! there are no start, tag or exception sections. wcc's element section lists the
//! functions whose address is taken; it is skipped, since every such use carries a
//! `TABLE_INDEX` relocation and the linker gives those functions their slots.

use crate::{Error, FuncType, ValType};
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;
use wasmparser::{KnownCustom, Linking, Parser, Payload, RelocationEntry, SymbolInfo, TypeRef};

/// One imported function: where from, and its type index.
pub(crate) struct FuncImport<'a> {
    pub module: &'a str,
    pub field: &'a str,
    pub ty: u32,
}

/// One imported global.
pub(crate) struct GlobalImport<'a> {
    pub module: &'a str,
    pub field: &'a str,
}

/// A section's payload and the relocations against it.
pub(crate) struct Patchable<'a> {
    pub index: u32,
    pub payload: &'a [u8],
    pub relocs: Vec<RelocationEntry>,
}

pub(crate) struct Object<'a> {
    pub types: Vec<FuncType>,
    pub func_imports: Vec<FuncImport<'a>>,
    pub global_imports: Vec<GlobalImport<'a>>,
    /// The type index of each defined function.
    pub funcs: Vec<u32>,
    pub code: Option<Patchable<'a>>,
    pub data: Option<Patchable<'a>>,
    pub data_count: u32,
    pub symbols: Vec<SymbolInfo<'a>>,
    /// Each data segment's alignment, as a power of two.
    pub segment_align: Vec<u32>,
}

/// `wasm[range]`, which a truncated object may not have.
fn slice(wasm: &[u8], range: core::ops::Range<usize>) -> Result<&[u8], Error> {
    wasm.get(range)
        .ok_or_else(|| Error::Parse("section past the end".into()))
}

fn parse_err(error: wasmparser::BinaryReaderError) -> Error {
    Error::Parse(error.message().to_string())
}

impl<'a> Object<'a> {
    pub fn parse(wasm: &'a [u8]) -> Result<Self, Error> {
        let mut object = Object {
            types: Vec::new(),
            func_imports: Vec::new(),
            global_imports: Vec::new(),
            funcs: Vec::new(),
            code: None,
            data: None,
            data_count: 0,
            symbols: Vec::new(),
            segment_align: Vec::new(),
        };
        let mut relocs: Vec<(u32, Vec<RelocationEntry>)> = Vec::new();
        let mut section = 0u32;
        let mut saw_linking = false;
        for payload in Parser::new(0).parse_all(wasm) {
            let payload = payload.map_err(parse_err)?;
            match payload {
                Payload::Version { .. } | Payload::End(_) => continue,
                Payload::CodeSectionEntry(_) => continue,
                Payload::TypeSection(reader) => object.read_types(reader)?,
                Payload::ImportSection(reader) => object.read_imports(reader)?,
                Payload::FunctionSection(reader) => {
                    for ty in reader {
                        object.funcs.push(ty.map_err(parse_err)?);
                    }
                }
                Payload::ExportSection(reader) => {
                    // wcc exports nothing with -c; exports are ours to choose.
                    if reader.count() != 0 {
                        return Err(Error::Unsupported("an export section".into()));
                    }
                }
                Payload::CodeSectionStart { range, .. } => {
                    object.code = Some(Patchable {
                        index: section,
                        payload: slice(wasm, range)?,
                        relocs: Vec::new(),
                    });
                }
                Payload::DataSection(reader) => {
                    object.data_count = reader.count();
                    object.data = Some(Patchable {
                        index: section,
                        payload: slice(wasm, reader.range())?,
                        relocs: Vec::new(),
                    });
                }
                Payload::DataCountSection { .. } | Payload::ElementSection(_) => {}
                Payload::CustomSection(custom) => match custom.as_known() {
                    KnownCustom::Linking(reader) => {
                        saw_linking = true;
                        object.read_linking(reader)?;
                    }
                    KnownCustom::Reloc(reader) => {
                        let entries = reader.entries();
                        let mut list = Vec::with_capacity(entries.count() as usize);
                        for entry in entries {
                            list.push(entry.map_err(parse_err)?);
                        }
                        relocs.push((reader.section_index(), list));
                    }
                    _ => {}
                },
                other => {
                    let name = match other {
                        Payload::StartSection { .. } => "a start section",
                        Payload::TagSection(_) => "exceptions (setjmp)",
                        Payload::MemorySection(_) => "a memory of its own",
                        Payload::TableSection(_) => "a table of its own",
                        Payload::GlobalSection(_) => "globals of its own",
                        _ => "an unknown section",
                    };
                    return Err(Error::Unsupported(name.into()));
                }
            }
            section += 1;
        }
        if !saw_linking {
            return Err(Error::Parse(
                "not a relocatable object (no linking section)".into(),
            ));
        }
        object.attach(relocs)?;
        Ok(object)
    }

    fn read_types(&mut self, reader: wasmparser::TypeSectionReader<'a>) -> Result<(), Error> {
        for group in reader.into_iter_err_on_gc_types() {
            let ty = group.map_err(parse_err)?;
            self.types.push(FuncType {
                params: ty
                    .params()
                    .iter()
                    .map(|v| val(*v))
                    .collect::<Result<_, _>>()?,
                results: ty
                    .results()
                    .iter()
                    .map(|v| val(*v))
                    .collect::<Result<_, _>>()?,
            });
        }
        Ok(())
    }

    fn read_imports(&mut self, reader: wasmparser::ImportSectionReader<'a>) -> Result<(), Error> {
        for import in reader {
            let import = import.map_err(parse_err)?;
            match import.ty {
                TypeRef::Func(ty) => self.func_imports.push(FuncImport {
                    module: import.module,
                    field: import.name,
                    ty,
                }),
                TypeRef::Global(global) => {
                    if !global.mutable || global.content_type != wasmparser::ValType::I32 {
                        return Err(Error::Unsupported(format!(
                            "imported global {}",
                            import.name
                        )));
                    }
                    self.global_imports.push(GlobalImport {
                        module: import.module,
                        field: import.name,
                    });
                }
                // The session's memory and table stand in for these.
                TypeRef::Memory(_) | TypeRef::Table(_) => {}
                TypeRef::Tag(_) => return Err(Error::Unsupported("exceptions (setjmp)".into())),
            }
        }
        Ok(())
    }

    fn read_linking(&mut self, reader: wasmparser::LinkingSectionReader<'a>) -> Result<(), Error> {
        if reader.version() != 2 {
            return Err(Error::Parse(format!(
                "linking version {}",
                reader.version()
            )));
        }
        for sub in reader.subsections() {
            match sub.map_err(parse_err)? {
                Linking::SymbolTable(map) => {
                    for symbol in map {
                        self.symbols.push(symbol.map_err(parse_err)?);
                    }
                }
                Linking::SegmentInfo(map) => {
                    for segment in map {
                        self.segment_align
                            .push(segment.map_err(parse_err)?.alignment);
                    }
                }
                Linking::InitFuncs(map) if map.count() == 0 => {}
                Linking::InitFuncs(_) => {
                    return Err(Error::Unsupported("constructors".into()));
                }
                Linking::ComdatInfo(_) | Linking::Unknown { .. } => {}
            }
        }
        Ok(())
    }

    /// Hands each relocation list to the section it patches.
    fn attach(&mut self, relocs: Vec<(u32, Vec<RelocationEntry>)>) -> Result<(), Error> {
        for (index, list) in relocs {
            let target = [self.code.as_mut(), self.data.as_mut()]
                .into_iter()
                .flatten()
                .find(|section| section.index == index);
            match target {
                Some(section) if section.relocs.is_empty() => section.relocs = list,
                Some(_) => return Err(Error::Parse("two reloc sections for one section".into())),
                None if list.is_empty() => {}
                None => {
                    return Err(Error::Unsupported(format!(
                        "relocations in section {index}"
                    )));
                }
            }
        }
        if self.segment_align.len() > self.data_count as usize {
            return Err(Error::Parse("more segment infos than data segments".into()));
        }
        self.segment_align.resize(self.data_count as usize, 0);
        Ok(())
    }

    /// The function type at `index`.
    pub fn ty(&self, index: u32) -> Result<&FuncType, Error> {
        self.types
            .get(index as usize)
            .ok_or_else(|| Error::Parse(format!("type index {index} out of range")))
    }
}

fn val(ty: wasmparser::ValType) -> Result<ValType, Error> {
    Ok(match ty {
        wasmparser::ValType::I32 => ValType::I32,
        wasmparser::ValType::I64 => ValType::I64,
        wasmparser::ValType::F32 => ValType::F32,
        wasmparser::ValType::F64 => ValType::F64,
        _ => return Err(Error::Unsupported("a SIMD or reference value".into())),
    })
}
