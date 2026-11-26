//! Nordic: Swedish and Finnish, which share one layout (Windows and X11's
//! `se` and `fi`): å, ä and ö, dead ´, `, ¨, ^ and ~. Danish and Norwegian
//! move æ and ø and are not here yet.

use crate::keymap::Accent::*;
use crate::keymap::Sym::{Char as C, Dead as D, None as N};
use crate::keymap::{Layout, Row};

pub const NORDIC: Layout = Layout {
    id: "nordic",
    name: "Svenska / Suomi",
    rows: ROWS,
    keypad_decimal: ',',
};

/// Usage, plain, Shift, AltGr; each row's comment is the key's US label.
#[rustfmt::skip]
const ROWS: &[Row] = &[
    (0x04, C('a'), C('A'), N), // A
    (0x05, C('b'), C('B'), N), // B
    (0x06, C('c'), C('C'), N), // C
    (0x07, C('d'), C('D'), N), // D
    (0x08, C('e'), C('E'), C('€')), // E
    (0x09, C('f'), C('F'), N), // F
    (0x0A, C('g'), C('G'), N), // G
    (0x0B, C('h'), C('H'), N), // H
    (0x0C, C('i'), C('I'), N), // I
    (0x0D, C('j'), C('J'), N), // J
    (0x0E, C('k'), C('K'), N), // K
    (0x0F, C('l'), C('L'), N), // L
    (0x10, C('m'), C('M'), C('µ')), // M
    (0x11, C('n'), C('N'), N), // N
    (0x12, C('o'), C('O'), N), // O
    (0x13, C('p'), C('P'), N), // P
    (0x14, C('q'), C('Q'), N), // Q
    (0x15, C('r'), C('R'), N), // R
    (0x16, C('s'), C('S'), N), // S
    (0x17, C('t'), C('T'), N), // T
    (0x18, C('u'), C('U'), N), // U
    (0x19, C('v'), C('V'), N), // V
    (0x1A, C('w'), C('W'), N), // W
    (0x1B, C('x'), C('X'), N), // X
    (0x1C, C('y'), C('Y'), N), // Y
    (0x1D, C('z'), C('Z'), N), // Z
    (0x1E, C('1'), C('!'), N), // 1
    (0x1F, C('2'), C('"'), C('@')), // 2
    (0x20, C('3'), C('#'), C('£')), // 3
    (0x21, C('4'), C('¤'), C('$')), // 4
    (0x22, C('5'), C('%'), C('€')), // 5
    (0x23, C('6'), C('&'), N), // 6
    (0x24, C('7'), C('/'), C('{')), // 7
    (0x25, C('8'), C('('), C('[')), // 8
    (0x26, C('9'), C(')'), C(']')), // 9
    (0x27, C('0'), C('='), C('}')), // 0
    (0x2D, C('+'), C('?'), C('\\')), // -
    (0x2E, D(Acute), D(Grave), N), // =
    (0x2F, C('å'), C('Å'), N), // [
    (0x30, D(Diaeresis), D(Circumflex), D(Tilde)), // ]
    (0x31, C('\''), C('*'), N), // \ (ANSI)
    (0x32, C('\''), C('*'), N), // ISO #
    (0x33, C('ö'), C('Ö'), N), // ;
    (0x34, C('ä'), C('Ä'), N), // '
    (0x35, C('§'), C('½'), N), // `
    (0x36, C(','), C(';'), N), // ,
    (0x37, C('.'), C(':'), N), // .
    (0x38, C('-'), C('_'), N), // /
    (0x64, C('<'), C('>'), C('|')), // ISO \
];
