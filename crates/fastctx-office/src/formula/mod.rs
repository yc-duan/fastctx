//! BIFF8 reverse-Polish expressions. Unknown tokens are errors for the
//! individual cell, never substituted with plausible formula text.

mod bytes;
mod functions;
mod number;
mod references;
#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result, bail, ensure};
pub(crate) use bytes::Bytes;

#[derive(Default)]
pub(crate) struct Context {
    pub names: Vec<std::result::Result<String, String>>,
    pub sheets: Vec<String>,
    pub books: Vec<Book>,
    pub xtis: Vec<(u16, u16, u16)>,
}

pub(crate) enum Book {
    Internal,
    Addin(Vec<String>),
    External { file: String, sheets: Vec<String> },
}

impl Context {
    fn sheet(&self, index: u16) -> Result<String> {
        let &(book, first, last) = self.xtis.get(index as usize).context("missing EXTERNSHEET entry")?;
        let book = self.books.get(book as usize).context("missing SUPBOOK")?;
        let sheets = match book {
            Book::Internal => &self.sheets,
            Book::External { sheets, .. } => sheets,
            Book::Addin(_) => bail!("sheet reference points to an add-in"),
        };
        if first == 0xffff || last == 0xffff {
            return Ok("#REF".into());
        }
        let first_name = sheets.get(first as usize).context("missing first sheet in XTI")?;
        let last_name = sheets.get(last as usize).context("missing last sheet in XTI")?;
        let name = if first == last { first_name.clone() } else { format!("{first_name}:{last_name}") };
        let simple =
            references::sheet_name(first_name) == *first_name && references::sheet_name(last_name) == *last_name;
        match book {
            Book::Internal => Ok(if simple { name } else { references::sheet_name(&name) }),
            Book::External { file, .. } => {
                ensure!(
                    file != " " && !file.chars().any(char::is_control) && !file.contains(['/', '\\', ':']),
                    "encoded external workbook path is not decoded"
                );
                let name = format!("[{file}]{name}");
                Ok(if simple && file.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.')) {
                    name
                } else {
                    format!("'{}'", name.replace('\'', "''"))
                })
            }
            Book::Addin(_) => unreachable!(),
        }
    }

    fn name(&self, index: u32) -> Result<String> {
        self.names
            .get(index.checked_sub(1).context("zero defined-name index")? as usize)
            .context("missing defined name")?
            .clone()
            .map_err(anyhow::Error::msg)
    }

    fn external_name(&self, index: u16, name: u32) -> Result<String> {
        let &(book, _, _) = self.xtis.get(index as usize).context("missing name XTI")?;
        match self.books.get(book as usize).context("missing SUPBOOK")? {
            Book::Internal => self.name(name),
            Book::Addin(names) => names
                .get(name.checked_sub(1).context("zero external-name index")? as usize)
                .cloned()
                .context("missing add-in name"),
            Book::External { .. } => bail!("external workbook name is not decoded"),
        }
    }
}

pub(crate) fn render(
    tokens: &[u8],
    extra: &[u8],
    origin: (u16, u16),
    shared: bool,
    context: &Context,
) -> Result<String> {
    let mut input = Bytes(tokens);
    let mut extra = Bytes(extra);
    let mut stack: Vec<String> = Vec::new();
    let mut spaces: Vec<(u8, usize)> = Vec::new();
    while !input.0.is_empty() {
        let byte = input.u8()?;
        ensure!(byte < 0x80, "invalid BIFF token 0x{byte:02X}");
        // Value/reference/array classes have identical payloads.
        let token = if byte >= 0x20 { (byte & 0x1f) | 0x20 } else { byte };
        let mut produced = !matches!(token, 0x19 | 0x26 | 0x27 | 0x28 | 0x29);
        match token {
            0x03..=0x11 => {
                let op = ["+", "-", "*", "/", "^", "&", "<", "<=", "=", ">=", ">", "<>", " ", ",", ":"]
                    [(token - 3) as usize];
                let right = pop(&mut stack)?;
                let left = pop(&mut stack)?;
                stack.push(format!("{left}{op}{right}"));
            }
            0x12..=0x15 => {
                let value = pop(&mut stack)?;
                stack.push(match token {
                    0x12 => format!("+{value}"),
                    0x13 => format!("-{value}"),
                    0x14 => format!("{value}%"),
                    _ => format!("({value})"),
                });
            }
            0x16 => stack.push(" ".into()),
            0x02 => bail!("data-table formula (PtgTbl) is not decoded"),
            0x17 => {
                let count = input.u8()? as usize;
                stack.push(format!("\"{}\"", input.string(count)?.replace('"', "\"\"")));
            }
            0x19 => {
                let kind = input.u8()?;
                let value = input.u16()?;
                match kind {
                    0x01 | 0x02 | 0x08 | 0x20 | 0x21 => {}
                    0x04 => {
                        input.take((usize::from(value) + 1) * 2)?;
                    }
                    0x10 => {
                        let value = pop(&mut stack)?;
                        stack.push(format!("SUM({value})"));
                        produced = true;
                    }
                    0x40 | 0x41 => {
                        let kind = value as u8;
                        ensure!(kind <= 6, "unknown formula whitespace type {kind}");
                        spaces.push((kind, (value >> 8) as usize));
                    }
                    _ => bail!("unsupported formula attribute 0x{kind:02X}"),
                }
            }
            0x1c => stack.push(error(input.u8()?)?.into()),
            0x1d => {
                let value = input.u8()?;
                ensure!(value <= 1, "invalid formula Boolean");
                stack.push(if value == 0 { "FALSE" } else { "TRUE" }.into());
            }
            0x1e => stack.push(input.u16()?.to_string()),
            0x1f => stack.push(number::text(input.f64()?)?),
            0x20 => {
                input.take(7)?;
                stack.push(array(&mut extra)?);
            }
            0x21 | 0x22 => {
                let given = if token == 0x22 { Some(input.u8()? as usize) } else { None };
                let index = input.u16()?;
                if index == 255
                    && let Some(count) = given
                {
                    ensure!(count > 0 && stack.len() >= count, "invalid external-function argument count");
                    let args = stack.split_off(stack.len() - count);
                    stack.push(format!("{}({})", args[0], args[1..].join(",")));
                } else {
                    let function = functions::get(index & 0x7fff, index & 0x8000 != 0)
                        .with_context(|| format!("unknown function index {index}"))?;
                    let count =
                        given.or(function.arguments).context("variable-arity function used as fixed-arity token")?;
                    ensure!(stack.len() >= count, "missing function operands");
                    let args = stack.split_off(stack.len() - count);
                    stack.push(format!("{}({})", function.name, args.join(",")));
                }
            }
            0x23 => stack.push(context.name(input.u32()?)?),
            0x24 | 0x2c => {
                let row = input.u16()?;
                let col = input.u16()?;
                ensure!(token != 0x2c || shared, "relative-offset reference outside a shared formula");
                ensure!(!shared || token == 0x2c || col & 0xc000 == 0, "relative Ref in shared formula");
                stack.push(references::cell(row, col, origin, token == 0x2c)?);
            }
            0x25 | 0x2d => {
                let rows = (input.u16()?, input.u16()?);
                let cols = (input.u16()?, input.u16()?);
                ensure!(token != 0x2d || shared, "relative-offset area outside a shared formula");
                ensure!(!shared || token == 0x2d || (cols.0 | cols.1) & 0xc000 == 0, "relative Area in shared formula");
                stack.push(references::area(rows, cols, origin, token == 0x2d)?);
            }
            0x26 => {
                input.take(6)?;
                let count = extra.u16()? as usize;
                extra.take(count * 8)?;
            }
            0x27 | 0x28 => {
                input.take(6)?;
            }
            0x29 => {
                input.take(2)?;
            }
            0x2a => {
                input.take(4)?;
                stack.push("#REF!".into());
            }
            0x2b => {
                input.take(8)?;
                stack.push("#REF!".into());
            }
            0x39 => {
                let index = input.u16()?;
                stack.push(context.external_name(index, input.u32()?)?);
            }
            0x3a => {
                ensure!(!shared, "3D reference in shared formula");
                let sheet = context.sheet(input.u16()?)?;
                let cell = references::cell(input.u16()?, input.u16()?, origin, false)?;
                stack.push(format!("{sheet}!{cell}"));
            }
            0x3b => {
                ensure!(!shared, "3D area in shared formula");
                let sheet = context.sheet(input.u16()?)?;
                let rows = (input.u16()?, input.u16()?);
                let cols = (input.u16()?, input.u16()?);
                stack.push(format!("{sheet}!{}", references::area(rows, cols, origin, false)?));
            }
            0x3c | 0x3d => {
                let sheet = context.sheet(input.u16()?)?;
                input.take(if token == 0x3c { 4 } else { 8 })?;
                stack.push(format!("{sheet}!#REF!"));
            }
            _ => bail!("unsupported BIFF token 0x{byte:02X}"),
        }
        if produced && !spaces.is_empty() {
            let mut leading = String::new();
            let mut closing = String::new();
            for (kind, count) in spaces.drain(..) {
                if matches!(kind, 2..=5) {
                    ensure!(token == 0x15, "parenthesis whitespace before non-parenthesis token");
                }
                let text = if matches!(kind, 1 | 3 | 5) { "\r" } else { " " }.repeat(count);
                if kind == 4 || kind == 5 {
                    closing.push_str(&text);
                } else {
                    leading.push_str(&text);
                }
            }
            let value = stack.last_mut().context("whitespace without expression")?;
            if !closing.is_empty() {
                value.insert_str(value.len() - 1, &closing);
            }
            value.insert_str(0, &leading);
        }
    }
    ensure!(stack.len() == 1, "formula leaves {} operands", stack.len());
    ensure!(spaces.is_empty(), "formula ends with unassigned whitespace");
    ensure!(extra.0.is_empty(), "unconsumed formula extra data");
    pop(&mut stack)
}

fn pop(stack: &mut Vec<String>) -> Result<String> {
    stack.pop().context("missing formula operand")
}

pub(crate) fn error(code: u8) -> Result<&'static str> {
    Ok(match code {
        0 => "#NULL!",
        7 => "#DIV/0!",
        15 => "#VALUE!",
        23 => "#REF!",
        29 => "#NAME?",
        36 => "#NUM!",
        42 => "#N/A",
        43 => "#GETTING_DATA",
        _ => bail!("unknown Excel error 0x{code:02X}"),
    })
}

fn array(input: &mut Bytes<'_>) -> Result<String> {
    let columns = usize::from(input.u8()?) + 1;
    let rows = usize::from(input.u16()?) + 1;
    let mut result = String::from("{");
    for row in 0..rows {
        if row > 0 {
            result.push(';');
        }
        for col in 0..columns {
            if col > 0 {
                result.push(',');
            }
            let text = match input.u8()? {
                1 => number::text(input.f64()?)?,
                2 => format!("\"{}\"", input.long_string()?.replace('"', "\"\"")),
                4 => {
                    let b = input.u8()?;
                    input.take(7)?;
                    ensure!(b <= 1, "invalid array Boolean");
                    if b == 0 { "FALSE" } else { "TRUE" }.into()
                }
                16 => {
                    let code = u8::try_from(input.u16()?).context("invalid array error code")?;
                    input.take(6)?;
                    error(code)?.into()
                }
                0 => bail!("empty array constant (SerNil) has no decoded formula text"),
                other => bail!("unsupported array constant type {other}"),
            };
            result.push_str(&text);
        }
    }
    result.push('}');
    Ok(result)
}
