#![feature(plugin)]

extern crate libc;
extern crate "bsdlike_getopts" as getopts;

extern crate regex;
#[plugin] #[no_link]
extern crate regex_macros;
#[macro_use]
extern crate macros;
extern crate collections;

use std::mem::{size_of, uninitialized, transmute};
use std::ptr::{copy_memory, zero_memory};
use std::sync::Arc;
use std::intrinsics;
use std::default::Default;
use std::os::MemoryMap;
use std::io;
use std::os::unix::AsRawFd;

pub use Endian::*;
//use std::ty::Unsafe;

pub fn copy_from_slice<T: Copy + Swap>(slice: &[u8], end: Endian) -> T {
    assert_eq!(slice.len(), size_of::<T>());
    unsafe {
        let mut t : T = uninitialized();
        copy_memory(&mut t, transmute(slice.as_ptr()), 1);
        t.bswap_from(end);
        t
    }
}

#[inline]
pub fn bswap64(x: u64) -> u64 {
    unsafe { intrinsics::bswap64(x) }
}
#[inline]
pub fn bswap32(x: u32) -> u32 {
    unsafe { intrinsics::bswap32(x) }
}
#[inline]
pub fn bswap16(x: u16) -> u16 {
    unsafe { intrinsics::bswap16(x) }
}

#[derive(Show, PartialEq, Eq, Copy)]
pub enum Endian {
    BigEndian,
    LittleEndian,
}

impl Default for Endian {
    fn default() -> Endian { BigEndian }
}

pub trait Swap {
    fn bswap(&mut self);
    fn bswap_from(&mut self, end: Endian) {
        if end == BigEndian { self.bswap() }
    }
}

macro_rules! impl_swap {
    ($ty:ty, $bsty:ty, $bsfun:ident) => (
        impl Swap for $ty {
            fn bswap(&mut self) {
                *self = $bsfun(*self as $bsty) as $ty;
            }
        }
    )
}

impl_swap!(u64, u64, bswap64);
impl_swap!(i64, u64, bswap64);
impl_swap!(u32, u32, bswap32);
impl_swap!(i32, u32, bswap32);
impl_swap!(u16, u16, bswap16);
impl_swap!(i16, u16, bswap16);

impl Swap for u8 {
    fn bswap(&mut self) {}
}
impl Swap for i8 {
    fn bswap(&mut self) {}
}

// dumb
macro_rules! impl_for_array{($cnt:expr) => (
    impl<T> Swap for [T; $cnt] {
        fn bswap(&mut self) {}
    }
)}
impl_for_array!(1);
impl_for_array!(2);
impl_for_array!(4);
impl_for_array!(16);
impl<T> Swap for Option<T> {
    fn bswap(&mut self) {}
}

pub unsafe fn zeroed_t<T>() -> T {
    let mut me : T = uninitialized();
    zero_memory(&mut me, 1);
    me
}

// TODO remove this
pub trait ToUi {
    fn to_ui(&self) -> usize;
}
impl ToUi for i32 { fn to_ui(&self) -> usize { *self as usize } }
impl ToUi for u32 { fn to_ui(&self) -> usize { *self as usize } }
impl ToUi for i16 { fn to_ui(&self) -> usize { *self as usize } }
impl ToUi for u16 { fn to_ui(&self) -> usize { *self as usize } }
impl ToUi for i8 { fn to_ui(&self) -> usize { *self as usize } }
impl ToUi for u8 { fn to_ui(&self) -> usize { *self as usize } }

pub trait X8 {}
impl X8 for u8 {}
impl X8 for i8 {}

pub fn trim_to_null<T: X8>(chs_: &[T]) -> &[u8] {
    let chs: &[u8] = unsafe { transmute(chs_) };
    match chs.iter().position(|c| *c == 0) {
        None => chs,
        Some(i) => chs.slice_to(i)
    }
}

pub fn from_cstr<T: X8>(chs_: &[T]) -> String {
    let truncated = trim_to_null(chs_);
    String::from_utf8_lossy(truncated).to_string()
}

#[derive(Clone, Default)]
pub struct MCRef {
    mm: Option<Arc<MemoryMap>>,
    off: usize,
    len: usize
}

unsafe impl Send for MCRef {}

impl MCRef {
    pub fn slice(&self, from: usize, to: usize) -> MCRef {
        let len = to - from;
        if from > self.len || len > self.len - from {
            panic!("MCRef::slice: bad slice");
        }
        MCRef { mm: self.mm.clone(), off: self.off + from, len: len }
    }
    pub fn get<'a>(&'a self) -> &'a [u8] {
        unsafe { std::slice::from_raw_buf::<u8>(
            transmute(&(self.mm.as_ref().unwrap().data().offset(self.off as isize) as *const u8)),
            self.len
        ) }
    }
    pub fn offset_in(&self, other: &MCRef) -> Option<usize> {
        match (&self.mm, &other.mm) {
            (&Some(ref mm1), &Some(ref mm2)) => {
                if (&**mm1 as *const MemoryMap) == (&**mm2 as *const MemoryMap) &&
                   other.off <= self.off && self.off <= other.off + other.len {
                    Some(self.off - other.off)
                } else { None }
            }
            _ => None
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
}

pub fn safe_mmap(fil: &mut std::io::File) -> MCRef {
    let oldpos = fil.tell().unwrap();
    fil.seek(0, io::SeekStyle::SeekEnd).unwrap();
    let size = fil.tell().unwrap();
    fil.seek(oldpos as i64, io::SeekStyle::SeekSet).unwrap();
    let rounded = std::cmp::max(size, 0x1000);
    let rsize = rounded as usize;
    if rsize as u64 != rounded {
        panic!("safe_mmap: file too big");
    }
    let fd = fil.as_raw_fd();
    let mm = MemoryMap::new(rsize, &[
        std::os::MapOption::MapReadable,
        std::os::MapOption::MapWritable,
        std::os::MapOption::MapFd(fd),
    ]).unwrap();
    assert!(mm.len() >= size as usize);
    MCRef { mm: Some(Arc::new(mm)), off: 0, len: size as usize }
}


pub fn do_getopts(args: &[String], min_expected_free: usize, max_expected_free: usize, optgrps: &mut Vec<getopts::OptGroup>) -> Option<getopts::Matches> {
    if let Ok(m) = getopts::getopts(args, optgrps.as_slice()) {
        if m.free.len() >= min_expected_free &&
            m.free.len() <= max_expected_free {
            return Some(m);
        }
    }
    None
}

pub fn do_getopts_or_panic(args: &[String], top: &str, min_expected_free: usize, max_expected_free: usize, optgrps: &mut Vec<getopts::OptGroup>) -> getopts::Matches {
    do_getopts(args, min_expected_free, max_expected_free, optgrps).unwrap_or_else(|:| { usage(top, optgrps); panic!(); })
}

pub fn usage(top: &str, optgrps: &mut Vec<getopts::OptGroup>) {
    optgrps.push(getopts::optflag("h", "help", "This help"));
    println!("{}", getopts::usage(top, optgrps.as_slice()));
}

pub fn exit() -> ! {
    unsafe { libc::exit(1) }
}

pub fn errlnb(s: &str) {
    // who needs speed
    std::io::stdio::stderr().write_line(s).unwrap();
}

pub fn errln(s: String) {
    errlnb(s.as_slice())
}

fn isprint(c: char) -> bool {
    let c = c as u32;
    if c >= 32 { c < 127 } else { (1 << c) & 0x3e00 != 0 }
}

pub fn shell_quote(args: &[String]) -> String {
    let mut sb = std::string::String::new();
    for arg_ in args.iter() {
        let arg = arg_.as_slice();
        if sb.len() != 0 { sb.push(' ') }
        if false { // XXX regex!(r"^[a-zA-Z0-9_-]+$").is_match(arg) {
            sb.push_str(arg);
        } else {
            sb.push('"');
            for ch_ in arg.as_bytes().iter() {
                let ch = *ch_ as char;
                if ch == '$' || ch == '`' || ch == '\\' || ch == '"' || ch == '\n' {
                    if ch == '\n' {
                        sb.push_str("\\n");
                    } else {
                        sb.push('\\');
                        sb.push(ch);
                    }
                } else if !isprint(ch) {
                    sb.push_str(format!("\\\\x{:02x}", *ch_).as_slice());
                } else {
                    sb.push(ch);
                }
            }
            sb.push('"');
        }
    }
    sb
}


pub trait OptionExt<T> {
    fn unwrap_ref(&self) -> &T;
}
impl<T> OptionExt<T> for Option<T> {
    fn unwrap_ref(&self) -> &T { self.as_ref().unwrap() }
}

pub trait VecStrExt {
    fn strings(&self) -> Vec<String>;
}
impl<T: std::string::ToString> VecStrExt for Vec<T> {
    fn strings(&self) -> Vec<String> { self.iter().map(|x| x.to_string()).collect() }
}

#[test]
fn test_branch() {
    let do_i = |i: usize| {
        branch!(if i == 1 {
            // Due to rustc being a piece of shit, ... I don't even.  You can only have one `let` (or any expression-as-statement), so make it count.  Maybe tomorrow I will figure this out.  Such a waste of time...
            type A = isize;
            type B = isize;
            let (b, c) = (7us, 8)
        } else {
            type A = usize;
            type B = usize;
            let (b, c) = (8us, 9)
        } then {
            println!("{}", (b + c) as A);
        })
    };
    for i in range(0, 2) {
        do_i(i)
    }
}

