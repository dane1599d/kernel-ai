//! Declarations at file scope: storage class, specifiers, the tagged bodies they
//! define, and each declarator's name and initializer; or a function definition.

use super::{
    DECL_WORDS, DIRECTIVE_INSIDE, Decl, Declarator, Func, Splitter, Storage, Tag, What,
    declarator_word, keyword, syntax,
};
use crate::Error;
use crate::lex::Kind;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

impl<'a> Splitter<'a> {
    pub(super) fn declaration(&mut self, first: usize) -> Result<What, Error> {
        let mut decl = Decl::default();
        let mut words: Vec<(usize, usize)> = Vec::new();
        self.specifiers(&mut decl, &mut words)?;
        decl.specifiers = words
            .iter()
            .map(|&(start, end)| &self.src[start..end])
            .collect::<Vec<_>>()
            .join(" ");
        loop {
            match self.t.get(self.pos) {
                None => break,
                Some(t) if t.kind == Kind::Directive => break,
                Some(_) => {}
            }
            if self.punct_at(self.pos) == Some(";") {
                self.pos += 1;
                decl.semi = true;
                break;
            }
            let (declarator, end) = self.declarator()?;
            if end == Some("{") {
                return self.function(first, decl, declarator);
            }
            decl.declarators.push(declarator);
            match end {
                Some(",") => self.pos += 1,
                Some(";") => {
                    self.pos += 1;
                    decl.semi = true;
                    break;
                }
                _ => break,
            }
        }
        decl.constant = decl.constant
            || self.t[first..self.pos]
                .iter()
                .any(|t| t.text(self.src) == "const");
        if decl.storage == Storage::Typedef {
            for d in &decl.declarators {
                self.local_types.push(d.name.clone());
            }
        }
        Ok(What::Decl(decl))
    }

    /// Reads storage classes, qualifiers, type words, tags and attributes.
    fn specifiers(
        &mut self,
        decl: &mut Decl,
        words: &mut Vec<(usize, usize)>,
    ) -> Result<(), Error> {
        let mut typed = false;
        while let Some(word) = self.word_at(self.pos) {
            let tok = self.t[self.pos];
            match word {
                "typedef" => decl.storage = Storage::Typedef,
                "extern" => decl.storage = Storage::Extern,
                "static" => decl.storage = Storage::Static,
                "struct" | "union" | "enum" => {
                    typed = true;
                    self.tag(decl, words)?;
                    continue;
                }
                "__attribute__" | "_Alignas" => {
                    let end = self.attribute()?;
                    words.push((tok.start, self.t[end - 1].end));
                    self.pos = end;
                    continue;
                }
                "void" | "char" | "short" | "int" | "long" | "float" | "double" | "signed"
                | "unsigned" | "_Bool" | "_Complex" => {
                    typed = true;
                    words.push((tok.start, tok.end));
                }
                w if DECL_WORDS.contains(&w) => words.push((tok.start, tok.end)),
                w if !typed && !keyword(w) => {
                    typed = true;
                    words.push((tok.start, tok.end));
                }
                _ => break,
            }
            self.pos += 1;
        }
        Ok(())
    }

    /// The index after an attribute word and its parenthesised group.
    fn attribute(&self) -> Result<usize, Error> {
        match self.punct_at(self.pos + 1) {
            Some("(") => self.group_end(self.pos + 1),
            _ => Err(syntax(self.t[self.pos].line, "an attribute needs its (")),
        }
    }

    /// `struct`, `union` or `enum`, an optional tag and an optional body.
    fn tag(&mut self, decl: &mut Decl, words: &mut Vec<(usize, usize)>) -> Result<(), Error> {
        let start = self.t[self.pos];
        let kind = start.text(self.src).to_string();
        self.pos += 1;
        let mut end = start.end;
        let mut name = None;
        if let Some(word) = self.word_at(self.pos).filter(|w| !keyword(w)) {
            name = Some(word.to_string());
            end = self.t[self.pos].end;
            self.pos += 1;
        }
        if self.punct_at(self.pos) != Some("{") {
            words.push((start.start, end));
            return Ok(());
        }
        let open = self.pos;
        let close = self.group_end(open)?;
        let enumerators = match kind.as_str() {
            "enum" => self.enumerators(open + 1, close - 1),
            _ => Vec::new(),
        };
        let body_end = self.t[close - 1].end;
        self.pos = close;
        match name {
            Some(name) => {
                let mut text = self.src[start.start..body_end].to_string();
                text.push(';');
                decl.tags.push(Tag {
                    kind,
                    name,
                    text,
                    enumerators,
                });
                words.push((start.start, end));
            }
            None => {
                decl.anonymous = true;
                decl.enumerators.extend(enumerators);
                words.push((start.start, body_end));
            }
        }
        Ok(())
    }

    /// The constant names in an `enum` body's tokens `from..to`.
    fn enumerators(&self, from: usize, to: usize) -> Vec<String> {
        let mut names = Vec::new();
        let mut depth = 0usize;
        let mut expect = true;
        for i in from..to {
            match self.punct_at(i) {
                Some("(" | "[" | "{") => depth += 1,
                Some(")" | "]" | "}") => depth = depth.saturating_sub(1),
                Some(",") if depth == 0 => {
                    expect = true;
                    continue;
                }
                _ => {}
            }
            if expect
                && depth == 0
                && let Some(word) = self.word_at(i)
            {
                names.push(word.to_string());
            }
            expect = false;
        }
        names
    }

    /// One declarator and its initializer; returns it with the punctuator that
    /// ended it (`,`, `;` or `{`), or `None` at the end of the entry.
    fn declarator(&mut self) -> Result<(Declarator, Option<&'a str>), Error> {
        let first = self.pos;
        let line = self.t[first].line;
        let mut name: Option<usize> = None;
        let mut function = false;
        let mut array = false;
        let mut depth = 0usize;
        let mut end = None;
        while let Some(tok) = self.t.get(self.pos).copied() {
            let text = tok.text(self.src);
            match tok.kind {
                Kind::Directive if depth == 0 => break,
                Kind::Directive => return Err(syntax(tok.line, DIRECTIVE_INSIDE)),
                Kind::Punct if depth == 0 && matches!(text, "=" | "," | ";" | "{") => {
                    end = Some(text);
                    break;
                }
                Kind::Punct => match text {
                    "(" | "[" | "{" => {
                        if text == "(" && name.is_some_and(|n| n + 1 == self.pos) {
                            function = true;
                        }
                        array |= text == "[" && name.is_some();
                        depth += 1;
                    }
                    ")" | "]" | "}" => {
                        depth = depth.checked_sub(1).ok_or_else(|| {
                            syntax(tok.line, "a closing bracket with nothing open")
                        })?;
                    }
                    _ => {}
                },
                Kind::Ident if text == "__attribute__" || text == "_Alignas" => {
                    self.pos = self.attribute()?;
                    continue;
                }
                Kind::Ident if name.is_none() && !keyword(text) && !declarator_word(text) => {
                    name = Some(self.pos);
                }
                _ => {}
            }
            self.pos += 1;
        }
        if depth > 0 {
            return Err(syntax(line, "a bracket is not closed"));
        }
        let Some(name) = name else {
            return Err(syntax(line, "a declaration needs a name"));
        };
        let text = self.src[self.t[first].start..self.t[self.pos - 1].end].to_string();
        let mut declarator = Declarator {
            name: self.text(name).to_string(),
            text,
            init: None,
            function,
            array,
        };
        if end == Some("=") {
            self.pos += 1;
            let (init, stop) = self.initializer(line)?;
            declarator.init = Some(init);
            end = stop;
        }
        Ok((declarator, end))
    }

    /// An initializer, to the `,` or `;` at depth 0.
    fn initializer(&mut self, line: u32) -> Result<(String, Option<&'a str>), Error> {
        let first = self.pos;
        let mut end = None;
        while let Some(tok) = self.t.get(self.pos).copied() {
            match (tok.kind, tok.text(self.src)) {
                (Kind::Directive, _) => break,
                (Kind::Punct, p @ ("," | ";")) => {
                    end = Some(p);
                    break;
                }
                (Kind::Punct, "(" | "[" | "{") => self.pos = self.group_end(self.pos)?,
                (Kind::Punct, ")" | "]" | "}") => {
                    return Err(syntax(tok.line, "a closing bracket with nothing open"));
                }
                _ => self.pos += 1,
            }
        }
        if self.pos == first {
            return Err(syntax(line, "= needs a value"));
        }
        let text = &self.src[self.t[first].start..self.t[self.pos - 1].end];
        Ok((text.to_string(), end))
    }

    /// A function definition: `declarator` is followed by its body at `self.pos`.
    fn function(
        &mut self,
        first: usize,
        decl: Decl,
        declarator: Declarator,
    ) -> Result<What, Error> {
        let line = self.t[first].line;
        if !decl.declarators.is_empty() || !declarator.function || declarator.init.is_some() {
            return Err(syntax(line, "a { where a declaration should end"));
        }
        if decl.storage == Storage::Typedef {
            return Err(syntax(line, "a typedef has no body"));
        }
        let open = self.pos;
        let proto_end = self.t[open - 1].end;
        let mut proto = self.src[self.t[first].start..proto_end].to_string();
        proto.push(';');
        let nullary = self.nullary(first, open, &declarator.name);
        self.pos = self.group_end(open)?;
        Ok(What::Func(Func {
            name: declarator.name,
            proto,
            local: decl.storage == Storage::Static,
            nullary,
        }))
    }

    /// Whether the parameter list after `name` in tokens `from..to` is `()` or
    /// `(void)`.
    fn nullary(&self, from: usize, to: usize, name: &str) -> bool {
        let Some(at) = (from..to).find(|&i| self.word_at(i) == Some(name)) else {
            return false;
        };
        matches!(
            (
                self.text_at(at + 1),
                self.text_at(at + 2),
                self.text_at(at + 3),
            ),
            (Some("("), Some(")"), _) | (Some("("), Some("void"), Some(")"))
        )
    }
}
