use super::decode::decode;
use super::*;
use sha2::{Digest, Sha256};

/// The guest, as it must read: what each instruction decodes to.
const LISTING: &str = "\
real 0x8000
cli
xor ax, ax
mov ds, ax
mov es, ax
lgdt [0x8040]
mov eax, cr0
or al, 0x1
mov cr0, eax
jmp 0x8:0x8046
protected 0x8046
mov ax, 0x10
mov ds, ax
mov es, ax
cld
mov edi, 0x9000
xor eax, eax
mov ecx, 0xc00
rep stosd
mov dword [0x9000], 0xa003
mov dword [0xa000], 0xb003
mov dword [0xb000], 0x83
mov eax, 0x20
mov cr4, eax
mov eax, 0x9000
mov cr3, eax
mov ecx, 0xc0000080
rdmsr
or eax, 0x100
wrmsr
mov eax, cr0
or eax, 0x80000000
mov cr0, eax
jmp 0x18:0x80ab
long 0x80ab
mov esi, 0x80bf
mov dx, 0x3f8
lodsb
test al, al
jz 0x80bc
out dx, al
jmp 0x80b4
hlt
jmp 0x80bc
";

/// The image's SHA-256: a change to any byte of the guest changes it.
const IMAGE_SHA256: &str = "ee237d99f0acd91ad4d8f539dbbe2de09edb02b7ac01e40e5d11438b1311c06f";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_guest_decodes_to_the_expected_instructions() {
    let guest = Guest::build();
    let mut listing = String::new();
    for code in &guest.code {
        let name = format!("{:?}", code.mode).to_lowercase();
        listing += &format!("{name} {:#x}\n", code.addr);
        let mut addr = code.addr;
        for insn in &code.insns {
            // The bytes written beside the text decode to that text, whole,
            // and sit in the image where the code says.
            let (len, text) = decode(code.mode, addr, &insn.bytes)
                .unwrap_or_else(|| panic!("{:02x?} does not decode", insn.bytes));
            assert_eq!((len, text.as_str()), (insn.bytes.len(), insn.text.as_str()));
            let at = (addr - LOAD) as usize;
            assert_eq!(guest.image[at..at + len], insn.bytes[..]);
            listing += &text;
            listing.push('\n');
            addr += len as u32;
        }
    }
    assert_eq!(listing, LISTING);
}

#[test]
fn the_image_is_one_fixed_flat_binary_that_ends_in_its_line() {
    let image = image();
    assert_eq!(hex(&Sha256::digest(&image)), IMAGE_SHA256);
    let text = format!("{EXPECTED_SERIAL}\n\0");
    assert!(image.ends_with(text.as_bytes()));
    assert!(image.len() < 0x1000, "{}", image.len());
}

/// The jumps and the string end where the image says: the message is the last
/// thing in the image, and the load loop reads it from there.
#[test]
fn the_loop_reads_the_message_that_ends_the_image() {
    let guest = Guest::build();
    let long = guest.code.last().unwrap();
    let want = LOAD + (guest.image.len() - EXPECTED_SERIAL.len() - 2) as u32;
    assert_eq!(long.insns[0].text, format!("mov esi, {want:#x}"));
}
