//! JSON (US-02.5): one segment per string or number value, with its RFC 6901 pointer, read as a
//! stream through a serde visitor. No value tree is built.

use std::fmt;
use std::io::BufReader;

use mk_kernel::Locator;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::content::{Format, ParseError, ParsedSegment, Place, Sink, Source};
use crate::encoding::{Decoded, detect};

/// Text in the encodings of NFR-COMP-06. Every string and number value becomes a segment at
/// its JSON Pointer (US-02.5 AC1); empty strings, booleans and nulls give none. Numbers are
/// stored as serde_json prints them (ASSUMPTION: `1.50` is stored as `1.5`). Invalid JSON fails
/// with the line and column of the error (US-02.5 AC2); nesting deeper than serde_json's limit
/// (128) fails with "JSON is nested too deeply".
pub(crate) fn parse(
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
) -> Result<(), ParseError> {
    let encoding = detect(input)?;
    sink.document(place.start(Format::Json, Vec::new()))?;
    let reader = BufReader::with_capacity(1 << 16, Decoded::new(input, encoding)?);
    let mut de = serde_json::Deserializer::from_reader(reader);
    let mut walk = Walk {
        sink,
        pointer: String::new(),
        failed: None,
    };
    let result = Value(&mut walk)
        .deserialize(&mut de)
        .and_then(|()| de.end());
    match result {
        Ok(()) => Ok(()),
        Err(e) => Err(walk.failed.take().unwrap_or_else(|| invalid(&e))),
    }
}

fn invalid(e: &serde_json::Error) -> ParseError {
    if e.to_string().starts_with("recursion limit exceeded") {
        return ParseError::new("json_too_deep", "JSON is nested too deeply");
    }
    if e.is_io() {
        return ParseError::new(
            "file_unreadable",
            format!("The file could not be read: {e}"),
        );
    }
    ParseError::new(
        "invalid_json",
        format!("Invalid JSON at line {}, column {}", e.line(), e.column()),
    )
}

struct Walk<'a> {
    sink: &'a mut dyn Sink,
    /// The pointer of the value being read.
    pointer: String,
    /// The sink's error, kept while serde unwinds with a stand-in.
    failed: Option<ParseError>,
}

impl Walk<'_> {
    fn emit<E: de::Error>(&mut self, text: String) -> Result<(), E> {
        let pointer = self.pointer.clone();
        let Some(seg) = ParsedSegment::whole(|span| Locator::JsonValue { pointer, span }, text)
        else {
            return Ok(());
        };
        self.sink.segment(seg).map_err(|e| {
            self.failed = Some(e);
            E::custom("output closed")
        })
    }

    /// Runs `f` with `token` added to the pointer.
    fn at<R>(&mut self, token: &str, f: impl FnOnce(&mut Self) -> R) -> R {
        let len = self.pointer.len();
        self.pointer.push('/');
        for c in token.chars() {
            match c {
                '~' => self.pointer.push_str("~0"),
                '/' => self.pointer.push_str("~1"),
                c => self.pointer.push(c),
            }
        }
        let r = f(self);
        self.pointer.truncate(len);
        r
    }
}

/// One value at the walk's current pointer.
struct Value<'w, 'a>(&'w mut Walk<'a>);

impl<'de> DeserializeSeed<'de> for Value<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Value<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<(), E> {
        self.0.emit(v.to_string())
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<(), E> {
        self.0.emit(v)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<(), E> {
        self.0.emit(v.to_string())
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<(), E> {
        self.0.emit(v.to_string())
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<(), E> {
        let text = serde_json::Number::from_f64(v).map_or_else(|| v.to_string(), |n| n.to_string());
        self.0.emit(text)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut i = 0usize;
        loop {
            let more = self
                .0
                .at(&i.to_string(), |w| seq.next_element_seed(Value(w)))?;
            if more.is_none() {
                return Ok(());
            }
            i += 1;
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            self.0.at(&key, |w| map.next_value_seed(Value(w)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::content::{Format, parse};
    use mk_kernel::Locator;

    fn values(json: &str) -> Vec<(String, String)> {
        let parsed = parse(Format::Json, json.as_bytes()).unwrap();
        parsed.documents[0]
            .segments
            .iter()
            .map(|s| match &s.locator {
                Locator::JsonValue { pointer, .. } => (pointer.clone(), s.text.clone()),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// US-02.5 AC1: every string and number value with its pointer; keys with `/` and `~` are
    /// escaped (RFC 6901).
    #[test]
    fn us_02_5_ac1_values_with_pointers() {
        let got = values(
            r#"{"customers":[{"name":"Anna","phone":"+44 20 7946 0000","n":12,"x":1.5,
                "ok":true,"none":null,"empty":""}],"a/b":{"~k":-3},"top":"t"}"#,
        );
        assert_eq!(
            got,
            vec![
                ("/customers/0/name".into(), "Anna".into()),
                ("/customers/0/phone".into(), "+44 20 7946 0000".into()),
                ("/customers/0/n".into(), "12".into()),
                ("/customers/0/x".into(), "1.5".into()),
                ("/a~1b/~0k".into(), "-3".into()),
                ("/top".into(), "t".into()),
            ]
        );
        assert_eq!(values(r#""root""#), vec![(String::new(), "root".into())]);
    }

    /// US-02.5 AC2: invalid JSON fails with the line and column; too deep and blank fail too
    /// (zero bytes is an empty document, as for every format).
    #[test]
    fn us_02_5_ac2_invalid_json_line_column() {
        let e = parse(Format::Json, b"{\n  \"a\": [1, 2,,]\n}").unwrap_err();
        assert_eq!(e.code, "invalid_json");
        assert_eq!(e.message, "Invalid JSON at line 2, column 14");
        let deep = "[".repeat(200) + &"]".repeat(200);
        let e = parse(Format::Json, deep.as_bytes()).unwrap_err();
        assert_eq!(e.message, "JSON is nested too deeply");
        let e = parse(Format::Json, b"\n").unwrap_err();
        assert_eq!(e.message, "Invalid JSON at line 2, column 0");
        assert_eq!(parse(Format::Json, b"").unwrap().documents.len(), 1);
        let e = parse(Format::Json, b"{} extra").unwrap_err();
        assert!(e.message.starts_with("Invalid JSON at line 1"), "{e}");
    }
}
