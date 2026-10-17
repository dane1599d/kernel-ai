//! Shrinks a user program for the boot drive: the kernel's loader reads only the
//! ELF header, the program headers and the loadable segments, so everything after
//! the last of them (debug info, section headers) is cut off. The line table is
//! built from the full file on the host before this.

/// Returns `elf` truncated after its last loadable byte, without section headers.
pub fn for_boot(elf: &[u8]) -> Result<Vec<u8>, String> {
    let read = |at: usize, len: usize| -> Result<u64, String> {
        let bytes = elf.get(at..at + len).ok_or("ELF header is cut short")?;
        let mut value = [0u8; 8];
        value[..len].copy_from_slice(bytes);
        Ok(u64::from_le_bytes(value))
    };
    if elf.get(..4) != Some(&[0x7F, b'E', b'L', b'F'][..]) || elf.get(4) != Some(&2) {
        return Err("not an ELF64 file".into());
    }
    let ph_offset = read(32, 8)?;
    let ph_size = read(54, 2)?;
    let ph_count = read(56, 2)?;
    let mut end = ph_offset + ph_size * ph_count;
    for index in 0..ph_count {
        let at = (ph_offset + index * ph_size) as usize;
        // PT_LOAD
        if read(at, 4)? == 1 {
            end = end.max(read(at + 8, 8)? + read(at + 32, 8)?);
        }
    }
    let end = usize::try_from(end).map_err(|_| "segment end out of range")?;
    let mut stripped = elf
        .get(..end)
        .ok_or("a segment lies past the end of the file")?
        .to_vec();
    // No section headers any more: e_shoff, e_shnum and e_shstrndx become zero.
    stripped[40..48].fill(0);
    stripped[60..64].fill(0);
    // Same segments, same contents, as the kernel will see them.
    let allowed = 0x1_0000..kabi::layout::HEAP_BASE;
    let before = uelf::parse(elf, allowed.clone()).map_err(|e| format!("{e:?}"))?;
    let after = uelf::parse(&stripped, allowed).map_err(|e| format!("{e:?}"))?;
    let same = before.entry == after.entry
        && before.segments() == after.segments()
        && before
            .segments()
            .iter()
            .all(|s| elf[s.file.clone()] == stripped[s.file.clone()]);
    if !same {
        return Err("stripping changed the loadable image".into());
    }
    Ok(stripped)
}

#[cfg(test)]
mod tests {
    use super::for_boot;

    #[test]
    fn rejects_non_elf_input() {
        assert!(for_boot(b"not an elf at all, just text").is_err());
        assert!(for_boot(&[0x7F, b'E', b'L', b'F', 2]).is_err());
    }
}
