//! Parser for the map file `ld.lld -Map` writes.
//!
//! ```text
//!      VMA      LMA     Size Align Out     In      Symbol
//!   200000   200000       1e     2 .pg_code
//!   200000   200000       1e     2         /path/x.o:(.text.paged_fn)
//!   200001   200001       1e     1                 paged_fn
//! ```
//!
//! Output sections start in the `Out` column, input sections (`file:(name)`,
//! archive members as `lib.rlib(member.o):(name)`) in the `In` column,
//! symbols in the `Symbol` column (lld prints them demangled). Linker-script
//! assignments appear in the first two columns with ` = ` and are skipped.

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSymbol {
    /// Address as printed (Thumb functions have bit 0 set).
    pub addr: u32,
    pub size: u32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSection {
    /// Output section it was placed in.
    pub out: String,
    /// Input file (`<internal>` for linker-generated sections).
    pub file: String,
    pub name: String,
    pub addr: u32,
    pub size: u32,
    pub align: u32,
    pub symbols: Vec<MapSymbol>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputSection {
    pub name: String,
    pub addr: u32,
    pub size: u32,
}

#[derive(Clone, Debug, Default)]
pub struct LinkMap {
    pub outputs: Vec<OutputSection>,
    pub inputs: Vec<InputSection>,
}

fn hex(s: &str) -> Result<u32> {
    let v = u64::from_str_radix(s, 16).with_context(|| format!("bad hex `{s}`"))?;
    u32::try_from(v).with_context(|| format!("address {v:#x} beyond 32 bits"))
}

pub fn parse(text: &str) -> Result<LinkMap> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    let col = |name: &str| header.find(name);
    let (Some(c_out), Some(c_in), Some(c_sym)) = (col(" Out "), col(" In "), col(" Symbol")) else {
        bail!("not an lld map file (header `{header}`)");
    };
    let (c_out, c_in, c_sym) = (c_out + 1, c_in + 1, c_sym + 1);
    let mut map = LinkMap::default();
    let mut out: Option<String> = None;
    let mut current: Option<usize> = None;
    for (n, line) in lines.enumerate() {
        if line.len() <= c_out {
            continue;
        }
        let (nums, rest) = line.split_at(c_out);
        let f: Vec<&str> = nums.split_whitespace().collect();
        let [vma, _lma, size, align] = f.as_slice() else {
            bail!("map line {}: `{line}`", n + 2);
        };
        let indent = rest.len() - rest.trim_start().len();
        let text = rest.trim();
        let addr = hex(vma)?;
        let size = hex(size)?;
        if indent == 0 {
            current = None;
            if text.contains(" = ") {
                continue;
            }
            out = Some(text.to_string());
            map.outputs.push(OutputSection { name: text.to_string(), addr, size });
        } else if indent == c_in - c_out {
            current = None;
            let Some(o) = &out else { continue };
            if text.contains(" = ") || !text.ends_with(')') {
                continue;
            }
            let Some(split) = text.rfind(":(") else { continue };
            map.inputs.push(InputSection {
                out: o.clone(),
                file: text[..split].to_string(),
                name: text[split + 2..text.len() - 1].to_string(),
                addr,
                size,
                align: align.parse().unwrap_or(1),
                symbols: Vec::new(),
            });
            current = Some(map.inputs.len() - 1);
        } else if indent == c_sym - c_out {
            if text.starts_with('$') || text.contains(" = ") {
                continue;
            }
            if let Some(i) = current {
                map.inputs[i].symbols.push(MapSymbol { addr, size, name: text.to_string() });
            }
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "     VMA      LMA     Size Align Out     In      Symbol
       0        0        0     1 YOTON_FLAT_PASS = 1
  200000   200000       1e     2 .pg_code
  200000   200000       1e     2         /p/t.o:(.text.paged_fn)
  200000   200000        0     1                 $t
  200001   200001       1e     1                 paged_fn
  100000   100000       22     2 .text
  100000   100000      128     4         /p/lib.rlib(m.o):(.init)
  100128   100128       d8     1         . = ALIGN(512)
  100200   100200      140     4         <internal>:(.ARM.exidx)
  100200   100200      140     1                 __vectors
       0        0       10     4 .rel.text
       0        0        8     4         t.o:(.rel.text._start)
";

    #[test]
    fn parses_outputs_inputs_symbols() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.outputs.len(), 3);
        assert_eq!(m.outputs[0], OutputSection { name: ".pg_code".into(), addr: 0x200000, size: 0x1e });
        assert_eq!(m.inputs.len(), 4);
        let i = &m.inputs[0];
        assert_eq!(
            (i.out.as_str(), i.file.as_str(), i.name.as_str()),
            (".pg_code", "/p/t.o", ".text.paged_fn")
        );
        assert_eq!((i.addr, i.size, i.align), (0x200000, 0x1e, 2));
        assert_eq!(i.symbols, vec![MapSymbol { addr: 0x200001, size: 0x1e, name: "paged_fn".into() }]);
        assert_eq!(m.inputs[1].file, "/p/lib.rlib(m.o)");
        assert_eq!(m.inputs[2].file, "<internal>");
        assert_eq!(m.inputs[2].symbols[0].name, "__vectors");
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse("Archive member included\n").is_err());
    }
}
