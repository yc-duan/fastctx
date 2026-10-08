//! Validate selected content streams before the engine's tolerant decoder.

use std::collections::HashSet;

use hayro::hayro_syntax::content::{TypedIter, UntypedIter, ops::TypedInstruction};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, Stream};
use hayro::hayro_syntax::page::{Page, Resources};
use hayro::hayro_syntax::xref::XRef;

pub(super) fn contents(page: &Page<'_>, mut visit: impl FnMut(&Resources<'_>)) -> Result<(), String> {
    // Hayro 0.8 skips undecodable Contents array members, and its Flate
    // decoder can return a successful empty or partial buffer on corruption.
    match page.raw().get::<Object<'_>>(b"Contents".as_slice()) {
        None if page.raw().contains_key(b"Contents") => Err("damaged PDF page: Contents could not be read".into()),
        None | Some(Object::Null(_)) => Ok(()),
        Some(Object::Stream(stream)) => check_stream(stream, page.xref()),
        Some(Object::Array(array)) => {
            for object in array.raw_iter() {
                match resolve(object, page.xref())? {
                    Object::Stream(stream) => check_stream(stream, page.xref())?,
                    _ => return Err("damaged PDF page: Contents contains a non-stream object".into()),
                }
            }
            Ok(())
        }
        _ => Err("damaged PDF page: Contents is not a stream or array".into()),
    }?;
    let mut pending = vec![(page_bytes(page), page.resources().clone(), HashSet::new())];
    while let Some((bytes, resources, ancestors)) = pending.pop() {
        visit(&resources);
        let mut operators = TypedIter::new(&bytes);
        let mut raw = UntypedIter::new(&bytes);
        while let Some(instruction) = raw.next() {
            // TypedIter uses None both for EOF and a malformed path operator.
            // Walking the raw operators alongside it distinguishes the two.
            let operator = operators
                .next()
                .ok_or_else(|| format!("damaged PDF content: invalid {} operator", instruction.operator))?;
            if let TypedInstruction::Fallback(operator) = operator {
                return Err(format!("PDF content has an unsupported or malformed {operator} operator"));
            }
            let TypedInstruction::XObject(object) = operator else {
                continue;
            };
            let stream = resources
                .get_x_object(object.0)
                .ok_or_else(|| format!("damaged PDF content: XObject {} could not be read", object.0.as_str()))?;
            if stream.dict().get::<Name<'_>>(b"Subtype".as_slice()).is_some_and(|name| name.as_ref() == b"Form") {
                let mut ancestors = ancestors.clone();
                if !ancestors.insert(stream.obj_id()) {
                    return Err("damaged PDF content: cyclic Form XObject".into());
                }
                check_stream(stream.clone(), page.xref())?;
                let child_resources = stream
                    .dict()
                    .get::<Dict<'_>>(b"Resources".as_slice())
                    .map(Resources::new)
                    .unwrap_or_else(|| resources.clone());
                let child =
                    stream.decoded().map_err(|error| format!("damaged PDF Form content: {error:?}"))?.into_owned();
                pending.push((child, child_resources, ancestors));
            }
        }
    }
    Ok(())
}

// Page::page_stream retains its decoded buffer for the lifetime of the syntax
// document. Validation needs it only until this page has been checked. Preserve
// Hayro's separator rule and its handling of a failed tolerant decode; the
// strict stream checks above still run before these bytes are interpreted.
fn page_bytes(page: &Page<'_>) -> Vec<u8> {
    if let Some(stream) = page.raw().get::<Stream<'_>>(b"Contents".as_slice()) {
        return stream.decoded().map(|data| data.into_owned()).unwrap_or_default();
    }
    let mut bytes = Vec::new();
    if let Some(streams) = page.raw().get::<Array<'_>>(b"Contents".as_slice()) {
        for stream in streams.iter::<Stream<'_>>() {
            if let Ok(data) = stream.decoded() {
                bytes.extend_from_slice(&data);
                bytes.push(b' ');
            }
        }
    }
    bytes
}

fn resolve<'a>(object: MaybeRef<Object<'a>>, xref: &'a XRef) -> Result<Object<'a>, String> {
    match object {
        MaybeRef::NotRef(object) => Ok(object),
        MaybeRef::Ref(reference) => xref
            .get::<Object<'_>>(reference.into())
            .ok_or_else(|| "damaged PDF content: unresolved array reference".into()),
    }
}

fn check_stream<'a>(stream: Stream<'a>, xref: &'a XRef) -> Result<(), String> {
    let dictionary = stream.dict();
    let filters = match dictionary.get::<Object<'_>>(b"Filter".as_slice()) {
        Some(Object::Name(name)) => vec![name],
        Some(Object::Array(array)) => {
            let mut names = Vec::new();
            for item in array.raw_iter() {
                match resolve(item, xref)? {
                    Object::Name(name) => names.push(name),
                    _ => return Err("damaged PDF content: Filter contains a non-name object".into()),
                }
            }
            names
        }
        None if dictionary.contains_key(b"Filter") => {
            return Err("damaged PDF content: Filter could not be read".into());
        }
        // ISO 32000-1, 7.3.9: a null dictionary value is an absent entry.
        None | Some(Object::Null(_)) => Vec::new(),
        _ => return Err("damaged PDF content: Filter is not a name or array".into()),
    };
    let mut data = stream.raw_data();
    for filter in &filters {
        data = match filter.as_ref() {
            b"FlateDecode" | b"Fl" => miniz_oxide::inflate::decompress_to_vec_zlib(&data)
                .map_err(|error| format!("damaged PDF content: invalid Flate stream ({:?})", error.status))?
                .into(),
            b"ASCII85Decode" | b"A85" => ascii85(&data)?.into(),
            b"ASCIIHexDecode" | b"AHx" => ascii_hex(&data)?.into(),
            b"LZWDecode" | b"LZW" | b"RunLengthDecode" | b"RL" if filters.len() == 1 => {
                return stream.decoded().map(|_| ()).map_err(|error| format!("damaged PDF content: {error:?}"));
            }
            _ => return Err(format!("PDF content filter {} is not supported", filter.as_str())),
        };
    }
    Ok(())
}

fn ascii_hex(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    let mut high = None;
    for byte in data.iter().copied().filter(|b| !b.is_ascii_whitespace()) {
        if byte == b'>' {
            break;
        }
        let digit = (byte as char).to_digit(16).ok_or("damaged PDF content: invalid ASCIIHex stream")? as u8;
        match high.take() {
            Some(high) => result.push(high * 16 + digit),
            None => high = Some(digit),
        }
    }
    if let Some(high) = high {
        result.push(high * 16);
    }
    Ok(result)
}

fn ascii85(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    let mut digits = 0;
    let mut value = 0u64;
    for byte in data.iter().copied().filter(|b| !b.is_ascii_whitespace()) {
        if byte == b'~' {
            break;
        }
        if byte == b'z' && digits == 0 {
            result.extend_from_slice(&[0; 4]);
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            return Err("damaged PDF content: invalid ASCII85 stream".into());
        }
        value = value * 85 + u64::from(byte - b'!');
        digits += 1;
        if digits == 5 {
            result.extend_from_slice(
                &u32::try_from(value).map_err(|_| "damaged PDF content: ASCII85 group overflows")?.to_be_bytes(),
            );
            digits = 0;
            value = 0;
        }
    }
    if digits == 1 {
        return Err("damaged PDF content: incomplete ASCII85 group".into());
    }
    if digits > 1 {
        for _ in digits..5 {
            value = value * 85 + 84;
        }
        let word = u32::try_from(value).map_err(|_| "damaged PDF content: ASCII85 group overflows")?.to_be_bytes();
        result.extend_from_slice(&word[..digits - 1]);
    }
    Ok(result)
}
