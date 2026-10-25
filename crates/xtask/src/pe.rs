//! A minimal PE (Windows) import-table reader.
//!
//! `xtask` is dependency-free, and the release runner has no guaranteed
//! `dumpbin`/`llvm-objdump` on `PATH` (the MSVC tools are only on `PATH` inside a
//! developer prompt). Reading the import directory is small enough to do here:
//! walk the DOS header, the PE/COFF headers, the section table and the import
//! (and delay-import) descriptors.
//!
//! Only the import *names* are needed by the closure walk; bound imports, API
//! sets and forwarders do not change which DLLs the loader must find.

/// The DLLs named in a PE image's import and delay-import tables.
pub fn imports(data: &[u8]) -> Result<Vec<String>, String> {
    let image = Image::parse(data)?;
    let mut names = Vec::new();

    // Directory 1 is the import table; 13 is the delay-load table.
    if let Some((rva, _size)) = image.directory(1) {
        read_imports(&image, rva, &mut names)?;
    }
    if let Some((rva, _size)) = image.directory(13) {
        read_delay_imports(&image, rva, &mut names)?;
    }

    names.sort();
    names.dedup();
    Ok(names)
}

/// One PE section header.
struct Section {
    virtual_size: u32,
    virtual_address: u32,
    raw_size: u32,
    raw_offset: u32,
}

/// A parsed PE image: the bytes plus the section/data-directory maps needed to
/// turn RVAs into file offsets.
struct Image<'a> {
    data: &'a [u8],
    sections: Vec<Section>,
    directories: Vec<(u32, u32)>,
}

impl<'a> Image<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, String> {
        if data.len() < 0x40 || &data[..2] != b"MZ" {
            return Err("not a PE image (no MZ header)".to_string());
        }
        let pe_offset = read_u32(data, 0x3c)? as usize;
        if read_u32(data, pe_offset)? != 0x0000_4550 {
            return Err("not a PE image (no PE signature)".to_string());
        }

        let coff = pe_offset + 4;
        let section_count = read_u16(data, coff + 2)? as usize;
        let optional_size = read_u16(data, coff + 16)? as usize;
        let optional = coff + 20;

        let magic = read_u16(data, optional)?;
        let directories_offset = match magic {
            0x010b => optional + 96,  // PE32
            0x020b => optional + 112, // PE32+
            other => return Err(format!("unknown PE optional-header magic {other:#x}")),
        };
        let directory_count_offset = match magic {
            0x010b => optional + 92,
            0x020b => optional + 108,
            _ => unreachable!(),
        };
        let directory_count = read_u32(data, directory_count_offset)?.min(16) as usize;
        let mut directories = Vec::with_capacity(directory_count);
        for index in 0..directory_count {
            let entry = directories_offset + index * 8;
            directories.push((read_u32(data, entry)?, read_u32(data, entry + 4)?));
        }

        // The section table follows the optional header.
        let section_table = optional + optional_size;
        let mut sections = Vec::with_capacity(section_count);
        for index in 0..section_count {
            let entry = section_table + index * 40;
            sections.push(Section {
                virtual_size: read_u32(data, entry + 8)?,
                virtual_address: read_u32(data, entry + 12)?,
                raw_size: read_u32(data, entry + 16)?,
                raw_offset: read_u32(data, entry + 20)?,
            });
        }

        Ok(Self {
            data,
            sections,
            directories,
        })
    }

    fn directory(&self, index: usize) -> Option<(u32, u32)> {
        self.directories
            .get(index)
            .copied()
            .filter(|(rva, size)| *rva != 0 && *size != 0)
    }

    /// Map a relative virtual address to a file offset through the section table.
    fn offset(&self, rva: u32) -> Result<usize, String> {
        for section in &self.sections {
            let span = section.virtual_size.max(section.raw_size);
            let end = section.virtual_address.saturating_add(span);
            if rva >= section.virtual_address && rva < end {
                let delta = (rva - section.virtual_address) as usize;
                let offset = section.raw_offset as usize + delta;
                if offset < self.data.len() {
                    return Ok(offset);
                }
                break;
            }
        }
        Err(format!("RVA {rva:#x} is not backed by the file"))
    }

    /// Read a NUL-terminated ASCII string at an RVA.
    fn string(&self, rva: u32) -> Result<String, String> {
        let start = self.offset(rva)?;
        let end = self.data[start..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|length| start + length)
            .unwrap_or_else(|| (start + 4096).min(self.data.len()));
        Ok(String::from_utf8_lossy(&self.data[start..end])
            .trim()
            .to_string())
    }
}

fn read_imports(image: &Image<'_>, table_rva: u32, names: &mut Vec<String>) -> Result<(), String> {
    let mut offset = image.offset(table_rva)?;
    // Each descriptor is 20 bytes; the table ends at an all-zero descriptor. The
    // directory's byte size is not reliable enough to stop on (some linkers
    // report only the first descriptor), so walk to the terminator with a cap.
    for _ in 0..4096 {
        if offset + 20 > image.data.len() {
            break;
        }
        let descriptor = &image.data[offset..offset + 20];
        if descriptor.iter().all(|byte| *byte == 0) {
            break;
        }
        let name_rva = read_u32(image.data, offset + 12)?;
        if name_rva != 0 {
            names.push(image.string(name_rva)?);
        }
        offset += 20;
    }
    Ok(())
}

fn read_delay_imports(
    image: &Image<'_>,
    table_rva: u32,
    names: &mut Vec<String>,
) -> Result<(), String> {
    let mut offset = image.offset(table_rva)?;
    // Each delay-load descriptor is 32 bytes; `szName` is at +4. MSVC emits
    // `grAttrs & 1` (the name is an RVA); a VA-relative name predates that and is
    // not resolved here.
    for _ in 0..4096 {
        if offset + 32 > image.data.len() {
            break;
        }
        let descriptor = &image.data[offset..offset + 32];
        if descriptor.iter().all(|byte| *byte == 0) {
            break;
        }
        let attributes = read_u32(image.data, offset)?;
        let name_rva = read_u32(image.data, offset + 4)?;
        if name_rva != 0 && attributes & 1 != 0 {
            names.push(image.string(name_rva)?);
        }
        offset += 32;
    }
    Ok(())
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| format!("PE header truncated at {offset:#x}"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| format!("PE header truncated at {offset:#x}"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::imports;

    /// Build a one-section PE32+ whose import directory names one DLL.
    fn image_with_import(name: &str) -> Vec<u8> {
        let pe_offset = 0x40usize;
        let optional_size = 0xf0usize;
        let section_table = pe_offset + 24 + optional_size; // 0x148
        let raw = 0x200usize;
        let mut data = vec![0u8; 0x300];

        data[0..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&(pe_offset as u32).to_le_bytes());
        data[pe_offset..pe_offset + 4].copy_from_slice(b"PE\0\0");

        let coff = pe_offset + 4;
        data[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // sections
        data[coff + 16..coff + 18].copy_from_slice(&(optional_size as u16).to_le_bytes());

        let optional = coff + 20;
        data[optional..optional + 2].copy_from_slice(&0x020bu16.to_le_bytes()); // PE32+
        data[optional + 108..optional + 112].copy_from_slice(&16u32.to_le_bytes());
        // Import directory (index 1) at optional+112+8.
        let import_entry = optional + 112 + 8;
        data[import_entry..import_entry + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        data[import_entry + 4..import_entry + 8].copy_from_slice(&20u32.to_le_bytes());

        data[section_table..section_table + 5].copy_from_slice(b".text");
        data[section_table + 8..section_table + 12].copy_from_slice(&0x200u32.to_le_bytes());
        data[section_table + 12..section_table + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        data[section_table + 16..section_table + 20].copy_from_slice(&0x200u32.to_le_bytes());
        data[section_table + 20..section_table + 24].copy_from_slice(&(raw as u32).to_le_bytes());

        // Import descriptor: name RVA 0x1040, then the zero terminator.
        data[raw + 12..raw + 16].copy_from_slice(&0x1040u32.to_le_bytes());
        let name_offset = raw + 0x40;
        data[name_offset..name_offset + name.len()].copy_from_slice(name.as_bytes());
        data
    }

    #[test]
    fn reads_the_import_directory() {
        let data = image_with_import("KERNEL32.dll");
        assert_eq!(imports(&data).unwrap(), vec!["KERNEL32.dll".to_string()]);
    }

    #[test]
    fn reads_multiple_descriptors_and_deduplicates() {
        // A second descriptor with the same name, then the terminator.
        let mut data = image_with_import("KERNEL32.dll");
        let raw = 0x200usize;
        data[raw + 20 + 12..raw + 20 + 16].copy_from_slice(&0x1040u32.to_le_bytes());
        data[raw + 20 + 20..raw + 20 + 24].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(imports(&data).unwrap(), vec!["KERNEL32.dll".to_string()]);
    }

    #[test]
    fn rejects_a_non_pe_file() {
        let error = imports(b"not an executable").unwrap_err();
        assert!(error.contains("MZ"), "{error}");
    }
}
