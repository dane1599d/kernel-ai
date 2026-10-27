//! Linking `kai:*` interfaces by canonical version (ADR 0003 section 2): an import
//! `ns:pkg/iface@ver` binds the host table for `0.minor` while the package is 0.x,
//! then for the major. Anything else fails with `Reason::Link` when the version is
//! staged. The tables are `tables.rs`, generated from `wit/`.

use alloc::string::String;
use alloc::vec::Vec;
use appabi::Ty;

/// A core value type in a generated signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreTy {
    I32,
    I64,
    F32,
    F64,
}

/// One function of a `kai:*` interface.
pub struct WitFunc {
    /// Such as `kai:app/log`.
    pub interface: &'static str,
    /// The canonical version: `0.1` while 0.x, then the major.
    pub version: &'static str,
    pub name: &'static str,
    /// The core signature, as the guest's import or export has it.
    pub core_params: &'static [CoreTy],
    pub core_results: &'static [CoreTy],
    /// The component types, for `appabi`.
    pub params: fn() -> Vec<Ty>,
    pub result: fn() -> Option<Ty>,
}

impl WitFunc {
    pub fn func(&self, export: String) -> appabi::Func {
        appabi::Func {
            export,
            params: (self.params)(),
            result: (self.result)(),
        }
    }
}

/// `ns:pkg/iface@ver` as (`ns:pkg/iface`, canonical version).
pub fn split(qualified: &str) -> Option<(&str, String)> {
    let (interface, version) = qualified.split_once('@')?;
    let mut parts = version.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    let canonical = if major == 0 {
        alloc::format!("0.{minor}")
    } else {
        alloc::format!("{major}")
    };
    Some((interface, canonical))
}

/// The table entry an import binds to, if any.
pub fn find<'a>(table: &'a [WitFunc], qualified: &str, name: &str) -> Option<&'a WitFunc> {
    let (interface, version) = split(qualified)?;
    table
        .iter()
        .find(|f| f.interface == interface && f.version == version && f.name == name)
}

pub fn core_matches(expected: &[CoreTy], actual: &[wasmi::ValType]) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(e, a)| {
            matches!(
                (e, a),
                (CoreTy::I32, wasmi::ValType::I32)
                    | (CoreTy::I64, wasmi::ValType::I64)
                    | (CoreTy::F32, wasmi::ValType::F32)
                    | (CoreTy::F64, wasmi::ValType::F64)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_bind_by_their_canonical_line() {
        assert_eq!(
            split("kai:app/log@0.1.0"),
            Some(("kai:app/log", String::from("0.1")))
        );
        assert_eq!(
            split("kai:app/log@0.1.7"),
            Some(("kai:app/log", String::from("0.1")))
        );
        assert_eq!(
            split("kai:app/log@1.4.2"),
            Some(("kai:app/log", String::from("1")))
        );
        assert_eq!(split("kai:app/log"), None);
        assert_eq!(split("kai:app/log@x.1.0"), None);
        let table = crate::tables::IMPORTS;
        assert!(find(table, "kai:app/log@0.1.3", "write").is_some());
        assert!(find(table, "kai:app/log@0.2.0", "write").is_none());
        assert!(find(table, "kai:app/log@0.1.0", "shout").is_none());
    }
}
