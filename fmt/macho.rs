#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]
#![feature(collections, libc, into_cow)]
#![feature(negate_unsigned)]
#[macro_use]
extern crate macros;
extern crate util;
extern crate exec;
extern crate bsdlike_getopts as getopts;
extern crate collections;
extern crate libc;
use std::default::Default;
use std::vec::Vec;
use std::mem::replace;
use std::mem::size_of;
use std::str::FromStr;
use std::borrow::IntoCow;
use util::{ToUi, VecStrExt, MCRef, Swap};
use macho_bind::*;
use exec::{arch, VMA, SymbolValue};
use std::{u64, u32};

#[path="../out/macho_bind.rs"]
mod macho_bind;

pub mod dyldcache;

// dont bother with the unions
deriving_swap!(
#[repr(C)]
#[derive(Copy)]
pub struct x_nlist {
    pub n_strx: uint32_t,
    pub n_type: uint8_t,
    pub n_sect: uint8_t,
    pub n_desc: int16_t,
    pub n_value: uint32_t,
}
);
deriving_swap!(
#[repr(C)]
#[derive(Copy)]
pub struct x_nlist_64 {
    pub n_strx: uint32_t,
    pub n_type: uint8_t,
    pub n_sect: uint8_t,
    pub n_desc: uint16_t,
    pub n_value: uint64_t,
}
);

pub fn u32_to_prot(ip: u32) -> exec::Prot {
    exec::Prot {
        r: (ip & VM_PROT_READ) != 0,
        w: (ip & VM_PROT_WRITE) != 0,
        x: (ip & VM_PROT_EXECUTE) != 0,
    }
}

fn file_array(buf: &MCRef, name: &str, off: u32, count: u32, elm_size: usize) -> MCRef {
    file_array_64(buf, name, off as u64, count as u64, elm_size)
}
fn file_array_64(buf: &MCRef, name: &str, mut off: u64, mut count: u64, elm_size: usize) -> MCRef {
    let elm_size = elm_size as u64;
    let buf_len = buf.len() as u64;
    if off > buf_len {
        errln!("warning: {} (offset {}, {} * {}b-sized elements) starts past end of file ({}))", name, off, count, elm_size, buf_len);
        off = 0;
        count = 0;
    } else if count > (buf_len - off) / elm_size {
        errln!("warning: {} (offset {}, {} * {}b-sized elements) extends past end of file ({})); truncating", name, off, count, elm_size, buf_len);
        count = (buf_len - off) / elm_size;
    }
    buf.slice(off as usize, (off + count * elm_size) as usize)
}


#[derive(Default, Debug, Copy, Clone)]
pub struct SymSubset { off: usize, count: usize }
#[derive(Default, Debug, Copy, Clone)]
pub struct RelSubset(usize, usize);

pub struct DscTabs {
    pub symtab: MCRef,
    pub strtab: MCRef,
    pub start: u32,
    pub count: u32,
}

#[derive(Default)]
pub struct MachO {
    pub eb: exec::ExecBase,
    pub is64: bool,
    pub mh: mach_header,
    pub load_commands: Vec<MCRef>,
    // old-style symbol table:
    pub nlist_size: usize,
    pub symtab: MCRef,
    pub strtab: MCRef,
    pub dsc_tabs: Option<DscTabs>,
    pub localsym: SymSubset,
    pub extdefsym: SymSubset,
    pub undefsym: SymSubset,
    pub toc: MCRef,
    pub modtab: MCRef,
    pub extrefsym: MCRef,
    pub indirectsym: MCRef,
    pub extrel: RelSubset,
    pub locrel: RelSubset,
    // new-style
    pub dyld_info_is_only: bool,
    pub dyld_rebase: MCRef,
    pub dyld_bind: MCRef,
    pub dyld_weak_bind: MCRef,
    pub dyld_lazy_bind: MCRef,
    pub dyld_export: MCRef,
}

#[derive(Hash, PartialEq, Eq, Debug)]
enum MachOAllotype {
    Static(&'static str),
    Segment(usize),
}

impl exec::Exec for MachO {
    fn get_exec_base<'a>(&'a self) -> &'a exec::ExecBase {
        &self.eb
    }

    fn get_symbol_list(&self, source: exec::SymbolSource) -> Vec<exec::Symbol> {
        if source == exec::SymbolSource::All {
            let mut out = Vec::new();
            let mut skip_redacted = false;
            if let Some(DscTabs { ref symtab, ref strtab, start, count }) = self.dsc_tabs {
                self.push_nlist_symbols(symtab.get(), strtab.get(), start as usize, count as usize, skip_redacted, &mut out);
                skip_redacted = true;
            }
            self.push_nlist_symbols(self.symtab.get(), self.strtab.get(), 0, self.symtab.len() / self.nlist_size, skip_redacted, &mut out);
            out
        } else {
            unimplemented!()
        }
    }

    fn as_any(&self) -> &std::any::Any { self as &std::any::Any }
}

fn mach_arch_desc(cputype: i32, cpusubtype: i32) -> Option<&'static str> {
    let cputype = cputype as u32;
    let cpusubtype = cpusubtype as u32;
    Some(match (cputype, cpusubtype & !0x80000000) {
        (CPU_TYPE_HPPA, CPU_SUBTYPE_HPPA_ALL) => "hppa",
        (CPU_TYPE_I386, CPU_SUBTYPE_I386_ALL) => "i386",
        (CPU_TYPE_X86_64, CPU_SUBTYPE_X86_64_ALL) => "x86_64",
        (CPU_TYPE_I860, CPU_SUBTYPE_I860_ALL) => "i860",
        (CPU_TYPE_MC680x0, CPU_SUBTYPE_MC680x0_ALL) => "m68k",
        (CPU_TYPE_MC88000, CPU_SUBTYPE_MC88000_ALL) => "m88k",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_ALL) => "ppc",
        (CPU_TYPE_POWERPC64, CPU_SUBTYPE_POWERPC_ALL) => "ppc64",
        (CPU_TYPE_SPARC, CPU_SUBTYPE_SPARC_ALL) => "sparc",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_ALL) => "arm",
        (CPU_TYPE_ANY, CPU_SUBTYPE_MULTIPLE) => "any",
        (CPU_TYPE_HPPA, CPU_SUBTYPE_HPPA_7100LC) => "hppa7100LC",
        (CPU_TYPE_MC680x0, CPU_SUBTYPE_MC68030_ONLY) => "m68030",
        (CPU_TYPE_MC680x0, CPU_SUBTYPE_MC68040) => "m68040",
        (CPU_TYPE_I386, CPU_SUBTYPE_486) => "i486",
        (CPU_TYPE_I386, CPU_SUBTYPE_486SX) => "i486SX",
        (CPU_TYPE_I386, CPU_SUBTYPE_PENT) => "pentium",
        (CPU_TYPE_I386, CPU_SUBTYPE_PENTPRO) => "pentpro",
        (CPU_TYPE_I386, CPU_SUBTYPE_PENTII_M3) => "pentIIm3",
        (CPU_TYPE_I386, CPU_SUBTYPE_PENTII_M5) => "pentIIm5",
        (CPU_TYPE_I386, CPU_SUBTYPE_PENTIUM_4) => "pentium4",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_601) => "ppc601",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_603) => "ppc603",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_603e) => "ppc603e",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_603ev) => "ppc603ev",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_604) => "ppc604",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_604e) => "ppc604e",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_750) => "ppc750",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_7400) => "ppc7400",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_7450) => "ppc7450",
        (CPU_TYPE_POWERPC, CPU_SUBTYPE_POWERPC_970) => "ppc970",
        (CPU_TYPE_POWERPC64, CPU_SUBTYPE_POWERPC_970) => "ppc970-64",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V4T) => "armv4t",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V5TEJ) => "armv5",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_XSCALE) => "xscale",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V6) => "armv6",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7) => "armv7",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7F) => "armv7f",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7S) => "armv7s",
        (CPU_TYPE_ARM, CPU_SUBTYPE_ARM_V7K) => "armv7k",
        (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64_ALL) => "arm64",
        (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64_V8) => "arm64v8",
        (CPU_TYPE_ANY, CPU_SUBTYPE_LITTLE_ENDIAN) => "little",
        (CPU_TYPE_ANY, CPU_SUBTYPE_BIG_ENDIAN) => "big",
        _ => return None,
    })
}

fn fixup_segment_overflow(seg: &mut exec::Segment, sixtyfour: bool) {
    if sixtyfour {
        if seg.vmsize > exec::VMA(u64::MAX) - seg.vmaddr {
            errln!("warning: vmaddr+vmsize overflow: {}+0x{:x}; truncating", seg.vmaddr, seg.vmsize);
            seg.vmsize = exec::VMA(u64::MAX) - seg.vmaddr;
        }
        if seg.filesize > u64::MAX - seg.fileoff {
            errln!("warning: fileoff+filesize overflow: 0x{:x}+0x{:x}; truncating", seg.fileoff, seg.filesize);
            seg.filesize = u64::MAX - seg.fileoff;
        }
    } else {
        if seg.vmsize > exec::VMA(u32::MAX as u64) - seg.vmaddr {
            errln!("warning: vmaddr+vmsize 32-bit overflow: {}+0x{:x}; we're ok though", seg.vmaddr, seg.vmsize);
        }
        if seg.filesize > u32::MAX as u64 - seg.fileoff {
            errln!("warning: fileoff+filesize 32-bit overflow: 0x{:x}+0x{:x}; we're ok though", seg.fileoff, seg.filesize);
        }

    }
}

fn seg_name_to_macho(seg: &Segment, error_pfx: &str) -> [libc::c_char; 16] {
    let mut name = if let Some(ref name) = seg.name { &name[..] } else { "" };
    if name.len() > 15 {
        errln!("warning: {} name '{}' is too long, truncating", error_pfx, name);
        name = &name[..15];
    }
    let mut segname: [libc::c_char; 16] = [0; 16];
    for (i, b) in name.bytes().enumerate() { segname[i] = b; }
    segname
}

impl MachO {
    pub fn new(mc: MCRef, do_lcs: bool, hdr_offset: usize) -> exec::ExecResult<MachO> {
        let mut me: MachO = Default::default();
        if hdr_offset >= std::usize::MAX - size_of::<mach_header>() { return exec::err(exec::ErrorKind::BadData, "truncated"); }
        let mut lc_off = hdr_offset + size_of::<mach_header>();
        {
            let buf = mc.get();
            if buf.len() < lc_off { return exec::err(exec::ErrorKind::BadData, "truncated"); }
            let magic: u32 = util::copy_from_slice(&buf[hdr_offset..hdr_offset+4], util::BigEndian);
            let is64; let end;
            match magic {
                0xfeedface => { end = util::BigEndian; is64 = false; }
                0xfeedfacf => { end = util::BigEndian; is64 = true; }
                0xcefaedfe => { end = util::LittleEndian; is64 = false; }
                0xcffaedfe => { end = util::LittleEndian; is64 = true; }
                _ => return exec::err(exec::ErrorKind::BadData, "bad magic")
            }
            me.eb.endian = end;
            me.is64 = is64;
            me.mh = util::copy_from_slice(&buf[hdr_offset..lc_off], end);
            // useless 'reserved' field
            if is64 { lc_off += 4; }
        }
        me.eb.buf = mc;
        me.parse_header();
        if do_lcs {
            me.parse_load_commands(lc_off);
        }
        Ok(me)
    }

    pub fn subtype_desc(&self) -> Option<&'static str> {
        mach_arch_desc(self.mh.cputype, self.mh.cpusubtype)
    }

    pub fn desc(&self) -> String {
        let ft_desc = match self.mh.filetype {
            MH_OBJECT => "object",
            MH_EXECUTE => "executable",
            MH_CORE => "core",
            MH_DYLIB => "dylib",
            MH_DYLINKER => "dylinker",
            MH_BUNDLE => "bundle",
            MH_DSYM => "dSYM",
            MH_KEXT_BUNDLE => "kext",
            _ => "<unknown filetype>"
        };
        let st_desc = match self.subtype_desc() {
            Some(d) => d.into_cow(),
            None => format!("<unknown cpu {}/{}>", self.mh.cputype, self.mh.cpusubtype).into_cow()
        };
        format!("Mach-O {}/{}", ft_desc, st_desc)
    }

    fn parse_header(&mut self) {
        self.eb.arch = match self.mh.cputype as u32 {
            CPU_TYPE_X86 => arch::X86,
            CPU_TYPE_X86_64 => arch::X86_64,
            CPU_TYPE_ARM => arch::ARM,
            CPU_TYPE_ARM64 => arch::AArch64,
            CPU_TYPE_POWERPC => arch::PowerPC,
            CPU_TYPE_POWERPC64 => arch::PowerPC,
            // Even if we don't know the arch, we can at least do something.
            _ => arch::UnknownArch,
        }
        // we don't really care about cpusubtype but could fill it in
    }

    fn parse_load_commands(&mut self, mut lc_off: usize) {
        self.nlist_size = if self.is64 { size_of::<nlist_64>() } else { size_of::<nlist>() };
        let end = self.eb.endian;
        let buf = &self.eb.buf;
        //let buf_len = buf.len();
        let mut segi: usize = 0;
        for lci in 0..self.mh.ncmds {
            let lc: load_command = util::copy_from_slice(&buf[lc_off..lc_off + 8], end);
            let lc_buf = buf.slice(lc_off, lc_off + lc.cmdsize.to_ui());
            self.load_commands.push(lc_buf);
            let lc_buf = lc_buf.get();
            let this_lc_off = lc_off;
            let mut do_segment = |is64: bool, segs: &mut Vec<exec::Segment>, sects: &mut Vec<exec::Segment>| {
                branch!(if is64 == true { // '== true' due to macro suckage
                    type segment_command_x = segment_command_64;
                    type section_x = section_64;
                } else {
                    type segment_command_x = segment_command;
                    type section_x = section;
                } then {
                    let mut off = size_of::<segment_command_x>();
                    let sc: segment_command_x = util::copy_from_slice(&lc_buf[..off], end);
                    let segprot = u32_to_prot(sc.initprot as u32);
                    let mut seg = exec::Segment {
                        vmaddr: VMA(sc.vmaddr as u64),
                        vmsize: sc.vmsize as u64,
                        fileoff: sc.fileoff as u64,
                        filesize: sc.filesize as u64,
                        name: Some(util::from_cstr(&sc.segname)),
                        prot: segprot,
                        seg_idx: None,
                        private: lci,
                    };
                    fixup_segment_overflow(&mut seg, is64);
                    segs.push(seg);
                    for _ in 0..sc.nsects {
                        let s: section_x = util::copy_from_slice(&lc_buf[off..off + size_of::<section_x>()], end);
                        let mut seg = exec::Segment {
                            vmaddr: VMA(s.addr as u64),
                            vmsize: s.size as u64,
                            fileoff: s.offset as u64,
                            filesize: s.size as u64,
                            name: Some(util::from_cstr(&s.sectname)),
                            prot: segprot,
                            seg_idx: Some(segi),
                            private: this_lc_off + off,
                        };
                        fixup_segment_overflow(&mut seg, is64);
                        sects.push(seg);
                        off += size_of::<section_x>();
                    }
                });
                segi += 1;
            };
            match lc.cmd {
                LC_SEGMENT => do_segment(false, &mut self.eb.segments, &mut self.eb.sections),
                LC_SEGMENT_64 => do_segment(true, &mut self.eb.segments, &mut self.eb.sections),
                LC_DYLD_INFO | LC_DYLD_INFO_ONLY => {
                    let di: dyld_info_command = util::copy_from_slice(&lc_buf[..size_of::<dyld_info_command>()], end);
                    self.dyld_rebase = self.file_array("dyld rebase info", di.rebase_off, di.rebase_size, 1);
                    self.dyld_bind = self.file_array("dyld bind info", di.bind_off, di.bind_size, 1);
                    self.dyld_weak_bind = self.file_array("dyld weak bind info", di.weak_bind_off, di.weak_bind_size, 1);
                    self.dyld_lazy_bind = self.file_array("dyld lazy bind info", di.lazy_bind_off, di.lazy_bind_size, 1);
                    self.dyld_export = self.file_array("dyld lazy bind info", di.export_off, di.export_size, 1);

                },
                LC_SYMTAB => {
                    let sy: symtab_command = util::copy_from_slice(&lc_buf[..size_of::<symtab_command>()], end);
                    self.symtab = self.file_array("symbol table", sy.symoff, sy.nsyms, self.nlist_size);
                    self.strtab = self.file_array("string table", sy.stroff, sy.strsize, 1);
                    //if sy.std::usize::MAX / nlist_size
                    //sy.symoff

                },
                LC_DYSYMTAB => {
                    let ds: dysymtab_command = util::copy_from_slice(&lc_buf[..size_of::<dysymtab_command>()], end);
                    self.localsym = SymSubset { off: ds.ilocalsym.to_ui(), count: ds.nlocalsym.to_ui() };
                    self.extdefsym = SymSubse { off: (ds.iextdefsym.to_ui(), count: ds.nextdefsym.to_ui() };
                    self.undefsym = SymSubset { off: (ds.iundefsym.to_ui(), count: ds.nundefsym.to_ui() };
                    self.toc = self.file_array("dylib table of contents", ds.tocoff, ds.ntoc, size_of::<dylib_table_of_contents>());
                    let dylib_module_size = if self.is64 { size_of::<dylib_module_64>() } else { size_of::<dylib_module>() };
                    self.modtab = self.file_array("module table", ds.modtaboff, ds.nmodtab, dylib_module_size);
                    self.extrefsym = self.file_array("referenced symbol table", ds.extrefsymoff, ds.nextrefsyms, size_of::<dylib_reference>());
                    self.indirectsym = self.file_array("'indirect symbol' table", ds.indirectsymoff, ds.nindirectsyms, 4);
                    self.extrel = RelSubset(ds.extreloff.to_ui(), ds.nextrel.to_ui());
                    self.locrel = RelSubset(ds.locreloff.to_ui(), ds.nlocrel.to_ui());
                },


                _ => ()
            }
            lc_off += lc.cmdsize.to_ui();
        }
    }

    fn file_array(&self, name: &str, off: u32, count: u32, elm_size: usize) -> MCRef {
        file_array(&self.eb.buf, name, off, count, elm_size)
    }


    fn push_nlist_symbols<'a>(&self, symtab: &[u8], strtab: &'a [u8], start: usize, count: usize, skip_redacted: bool, out: &mut Vec<exec::Symbol<'a>>) {
        let mut off = start * self.nlist_size;
        for _ in start..start+count {
            let slice = &symtab[off..off + self.nlist_size];
            branch!(if self.is64 == true {
                type nlist_x = x_nlist_64;
            } else {
                type nlist_x = x_nlist;
            } then {
                let nl: nlist_x = util::copy_from_slice(slice, self.eb.endian);
                let n_type_field = nl.n_type as u32;
                let n_desc_field = nl.n_desc as u32;
                let _n_pext = (n_type_field & N_PEXT) != 0;
                let _n_stab = (n_type_field & N_STAB) >> 5;
                let n_type = n_type_field & N_TYPE;
                let weak = (n_desc_field & (N_WEAK_REF | N_WEAK_DEF)) != 0;
                let public = (n_type_field & N_EXT) != 0;
                let name = util::trim_to_null(&strtab[nl.n_strx.to_ui()..]);
                let vma = VMA(nl.n_value as u64);
                let vma = if n_desc_field & N_ARM_THUMB_DEF != 0 { vma | 1 } else { vma };
                let val =
                    if n_desc_field & N_SYMBOL_RESOLVER != 0 {
                        SymbolValue::Resolver(vma)
                    } else if n_type == N_UNDF {
                        SymbolValue::Undefined
                    } else if n_type == N_INDR {
                        assert!(nl.n_value <= 0xfffffffe);
                        let indr_name = util::trim_to_null(&strtab[nl.n_value as usize..]);
                        SymbolValue::ReExport(indr_name)

                    } else {
                        SymbolValue::Addr(vma)
                    };
                if skip_redacted && name == b"<redacted>" { continue; }
                out.push(exec::Symbol {
                    name: name,
                    is_public: public,
                    is_weak: weak,
                    val: val,
                    private: off,
                })
            });
            off += self.nlist_size;
        }
    }
    pub fn reallocate(&self) {
        let _ = self.update_cmds();
    }

    fn update_cmds(&self, allocations: &HashMap<MachOAllotype, usize>, symtab_allocations: (usize, usize, usize)) -> Vec<Vec<u8>> {
        let mut cmds: Vec<Vec<u8>> = Vec::new();
        let (existing_segs, extra_segs) = self.update_seg_cmds();
        let mut lci = 0;
        let mut insert_extra_segs_idx: Option<usize> = None;
        let mut by_id: HashMap<u32, Option<Vec<u8>>> = HashMap::new();;

        let mut dyld_info_cmd_id = LC_DYLD_INFO_ONLY;
        for cmd in self.load_commands {
            let cmd = cmd.get();
            let cmd_id: u32 = util::copy_from_slice(&cmd[..4], self.eb.endian);
            if cmd_id == LC_DYLD_INFO {
                dyld_info_cmd_id = cmd_id;
            }
        }

        by_id.insert(dyld_info_cmd_id, util::copy_to_new_vec(&dyld_info_command {
            cmd: dyld_info_cmd_id,
            cmdsize: size_of::<dyld_info_command>(),
            rebase_off: allocations[MachOAllotype::Static("dyld_rebase")],
            rebase_size: self.dyld_rebase.len(),
            bind_off: allocations[MachOAllotype::Static("dyld_bind")],
            bind_size: self.dyld_bind.len(),
            weak_bind_off: allocations[MachOAllotype::Static("dyld_weak_bind")],
            weak_bind_size: self.dyld_weak_bind.len(),
            lazy_bind_off: allocations[MachOAllotype::Static("dyld_lazy_bind")],
            lazy_bind_size: self.dyld_lazy_bind.len(),
            export_off: allocations[MachOAllotype::Static("dyld_export")],
            export_size: self.dyld_export.len(),
        }, self.eb.endian));
        ^^^^^^
        I bet this whole thing can be done cleverer, by putting this in one place.
        vvvvvv

        by_id.insert(LC_SYMTAB, util::copy_to_new_vec(&symtab_command {
            cmd: LC_SYMTAB,
            cmdsize: size_of::<symtab_command>(),
            symoff: allocations[MachOAllotype::Static("symtab")],
            nsyms: self.symtab.len() / if self.is64 { size_of::<nlist_64>() } else { size_of::<nlist>() },
            stroff: allocations[MachOAllotype::Static("strtab")],
            strsize: self.strtab.len(),
        }, self.eb.endian));

        by_id.insert(LC_DYSYMTAB, util::copy_to_new_vec(&dysymtab_command {
            cmd: LC_DYSYMTAB,
            cmdsize: size_of::<dysymtab_command>(),
            ilocalsym: symtab_allocations.0,
            nlocalsym: self.localsym.count,
            iextdefsym: symtab_allocations.1,
            nextdefsym: self.extdefsym.count,
            iundefsym: symtab_allocations.2,
            nundefsym: self.undefsym.count,
            tocoff: allocations[MachOAllotype::Static("toc")],
            ntoc: self.toc.len() / size_of::<dylib_table_of_contents>(),
            modtaboff: 
            nmodtab: self.toc.len() / if self.is64 { size_of::<dylib_module_64>() } else { size_of::<dylib_module>() };

        }, self.eb.endian));

        for cmd in self.load_commands {
            let cmd = cmd.get();
            let cmd_id: u32 = util::copy_from_slice(&cmd[..4], self.eb.endian);
            if cmd_id != LC_SEGMENT && cmd_id != LC_SEGMENT_64 {
                insert_extra_segs_idx = cmds.len();
            }
            match cmd_id {
                LC_SEGMENT | LC_SEGMENT_64 => {
                    if let Some(new_cmd) = existing_segs.remove(lci) {
                        cmds.push(new_cmd);
                    }
                },
                _ => {
                    if let (_, new_cmd_o) = by_id.remove(cmd_id) {
                        if let Some(new_cmd) = new_cmd_o { cmds.push(new_cmd); }
                    } else {
                        cmds.push(cmd.to_owned());
                    }
                },
            }
            lci += cmd.len();
        }
        for (cmd_id, new_cmd_o) in by_id.into_iter() {
            if let Some(new_cmd) = new_cmd_o { cmds.push(new_cmd); }
        }
        let mut insert_extra_segs_idx = insert_extra_segs_idx.unwrap_or(cmds.len());
        for (_, new_cmd) in existing_segs.drain() {
            cmds.insert(insert_extra_segs_idx, new_cmd);
            insert_extra_segs_idx += 1;
        }
        for new_cmd in extra_segs.move_iter() {
            cmds.insert(insert_extra_segs_idx, new_cmd);
            insert_extra_segs_idx += 1;
        }
        cmds
    }

    fn update_seg_cmds(&self) -> (HashMap<usize, Vec<u8>>, Vec<Vec<u8>>) {
        let mut existing_segs = HashMap::new();
        let mut extra_segs = Vec::new();
        for (segi, seg) in self.eb.segments.enumerate() {
            let lci = seg.private;
            let cmd = if self.is64 { LC_SEGMENT_64 } else { LC_SEGMENT };
            let sects: Vec<&exec::Segment> = self.eb.segments.map(|seg| seg.seg_idx == Some(segi)).collect();
            let nsects = sects.len();
            let segname = seg_name_to_macho(&seg, "MachO::reallocate: segment");
            let mut new_cmd = Vec::<u8>::new();
            let olcbuf = if lci != usize::MAX { Some(self.load_commands[lci].get()) } else { None }
            branch!(if self.is64 {
                type segment_command_x = segment_command_64;
                type section_x = section_64;
                type size_x = u64;
            } else {
                type segment_command_x = segment_command;
                type section_x = section;
                type size_x = u32;
            } then {
                let mut sc: segment_command_x = if let Some(ref lcbuf) = olcbuf {
                    util::copy_from_slice(&lcbuf[..size_of::<segment_command_x>()], self.eb.endian)
                } else {
                    segment_command_x {
                        maxprot: 7,
                        ..Default()
                    }
                };
                sc.cmd = cmd;
                sc.cmdsize = size_of::<segment_command_x>() + nsects * size_of::<section_x>();
                sc.segname = segname;
                assert!(seg.vmaddr.0 <= size_x::MAX);
                sc.vmaddr = seg.vmaddr.0 as size_x;
                sc.vmsize = seg.vmsize as size_x;
                sc.fileoff = seg.fileoff as size_x;
                sc.filesize = seg.filesize as size_x;
                sc.initprot = 0;
                if seg.prot.r { sc.initprot |= VM_PROT_READ; }
                if seg.prot.w { sc.initprot |= VM_PROT_WRITE; }
                if seg.prot.x { sc.initprot |= VM_PROT_EXECUTE; }

                seg.nsects = nsects;
                util::copy_to_vec(&mut new_cmd, &seg, self.eb.endian);

                for (secti, sect) in sects.enumerate() {
                    let mut snc: section_x = if let Some(ref lcbuf) = olcbuf {
                        let off = secti * size_of::<section_x>();
                        util::copy_from_slice(&lcbuf[off..off+size_of::<section_x>()], self.eb.endian)
                    } else {
                        Default()
                    };
                    snc.segname = segname;
                    snc.sectname = seg_name_to_macho(&sect, "MachO::reallocate: section");
                    snc.addr = sect.vmaddr.0 as size_x;
                    snc.size = sect.vmsize as size_x;
                    if sect.filesize != sect.vmsize {
                        errln!("warning: MachO::reallocate: section {} filesize != vmsize, using vmsize", sect.name);
                    }
                    snc.offset = sect.fileoff as size_x;
                    util::copy_to_vec(&mut new_cmd, &sect, self.eb.endian);
                }
            })
            if lci == usize::MAX {
                extra_segs.push(new_cmd);
            } else {
                existing_segs.insert(lci, new_cmd);
            }
        }
        (existing_segs, extra_segs)
    }
}

#[derive(Copy, Clone)]
pub struct MachOProber;

impl exec::ExecProber for MachOProber {
    fn name(&self) -> &str {
        "macho"
    }
    fn probe(&self, _eps: &Vec<&'static exec::ExecProber>, buf: MCRef) -> Vec<exec::ProbeResult> {
        if let Ok(m) = MachO::new(buf, false, 0) {
            vec!(exec::ProbeResult {
                desc: m.desc(),
                arch: m.eb.arch,
                likely: true,
                cmd: vec!("macho".to_string()),
            })
        } else {
            vec!()
        }
    }
   fn create(&self, _eps: &Vec<&'static exec::ExecProber>, buf: MCRef, args: Vec<String>) -> exec::ExecResult<(Box<exec::Exec>, Vec<String>)> {
        let m = util::do_getopts_or_panic(&*args, "macho ...", 0, std::usize::MAX, &mut vec!(
            // ...
        ));
        let mo: MachO = try!(MachO::new(buf, true, 0));
        Ok((Box::new(mo) as Box<exec::Exec>, m.free))
    }
}

#[derive(Copy, Clone)]
pub struct FatMachOProber;

impl FatMachOProber {
    fn probe_cb(&self, mc: &MCRef, cb: &mut FnMut(u64, fat_arch)) -> bool {
        let buf = mc.get();
        if buf.len() < 8 { return false }
        let fh: fat_header = util::copy_from_slice(&buf[..8], util::BigEndian);
        if fh.magic != FAT_MAGIC as u32 { return false }
        let nfat = fh.nfat_arch as u64;
        let mut off: usize = 8;
        if (buf.len() as u64) < (off as u64) + (nfat * size_of::<fat_arch>() as u64) {
            errln!("fatmacho: no room for {} fat archs", nfat);
            return false;
        }
        for i in 0..nfat {
            let fa: fat_arch = util::copy_from_slice(&buf[off..off + size_of::<fat_arch>()], util::BigEndian);
            if (fa.offset as u64) + (fa.size as u64) > (buf.len() as u64) {
                errln!("fatmacho: bad arch cputype={},{} offset={} size={} (truncated?)",
                       fa.cputype, fa.cpusubtype, fa.offset, fa.size);
            } else {
                cb(i, fa);
            }
            off += size_of::<fat_arch>();
        }
        true
    }
}

impl exec::ExecProber for FatMachOProber {
    fn name(&self) -> &str {
        "fat"
    }
    fn probe(&self, eps: &Vec<exec::ExecProberRef>, mc: MCRef) -> Vec<exec::ProbeResult> {
        let mut result = Vec::new();
        let ok = self.probe_cb(&mc, &mut |i, fa| {
            let arch = match mach_arch_desc(fa.cputype, fa.cpusubtype) {
                Some(desc) => desc.to_string(),
                None => format!("{}", i),
            };
            let off = fa.offset.to_ui();
            let size = fa.size.to_ui();
            for pr in exec::probe_all(eps, mc.slice(off, off + size)).into_iter() {
                let npr = exec::ProbeResult {
                    desc: format!("(slice #{}) {}", i, pr.desc),
                    arch: pr.arch,
                    likely: pr.likely,
                    cmd: vec!("fat", "--arch", &*arch).strings() + &*pr.cmd,
                };
                result.push(npr);
            }
        });
        if !ok { return vec!()}
        result
    }

    fn create(&self, eps: &Vec<exec::ExecProberRef>, mc: MCRef, args: Vec<String>) -> exec::ExecResult<(Box<exec::Exec>, Vec<String>)> {
        let top = "fat (--arch ARCH | -s SLICE)";
        let mut optgrps = vec!(
            getopts::optopt("", "arch", "choose by arch (OS X standard names)", "arch"),
            getopts::optopt("s", "slice", "choose by slice number", ""),
        );
        let mut m = util::do_getopts_or_panic(&*args, top, 0, std::usize::MAX, &mut optgrps);
        let slice_num = m.opt_str("slice");
        let arch = m.opt_str("arch");
        if slice_num.is_some() == arch.is_some() {
            // TODO
            util::usage(top, &mut optgrps);
            panic!();
        }
        let slice_i = slice_num.map_or(0u64, |s| FromStr::from_str(&*s).unwrap());
        let mut result = None;
        let ok = self.probe_cb(&mc, &mut |i, fa| {
            if if let (&None, &Some(ref arch_)) = (&result, &arch) {
                mach_arch_desc(fa.cputype, fa.cpusubtype).map_or(false, |d| d == &**arch_)
            } else {
                i == slice_i
            }
            {
                let off = fa.offset.to_ui();
                let size = fa.size.to_ui();
                result = Some(exec::create(eps, mc.slice(off, off + size), replace(&mut m.free, vec!())));
            }
        });
        if !ok {
            return exec::err(exec::ErrorKind::BadData, "invalid fat mach-o");
        }
        match result {
            Some(e) => e,
            None => exec::err(exec::ErrorKind::Other, "fat arch matching command line not found")
        }
    }
}


//#[test]

