//! NSKeyedArchiver decoding .
//!
//! The archive is parsed as a binary property list with the `plist` crate, then `$top["root"]`
//! is resolved into an owned [`Object`] tree. UID references are followed. Foundation classes
//! (NSArray, NSDictionary, NSString, NSData, NSUUID) become plain values; any other class
//! becomes an [`ArchivedObject`].
//!
//! Differences from the Python version: the result is a tree, not a shared graph. A reference
//! to an object that is still being decoded (a cycle such as `layer.document -> document`)
//! resolves to [`Object::Null`]. Other repeated references are expanded again. The property list
//! is checked with [`limits::check_plist`] before it is parsed, and the decoded tree's nesting
//! depth (references and inline collections together), node count and string and data bytes
//! are limited ([`limits::MAX_ARCHIVE_DEPTH`], [`limits::MAX_ARCHIVE_NODES`],
//! [`limits::MAX_ARCHIVE_BYTES`]).

use std::collections::BTreeMap;

use plist::{Dictionary, Value};

use crate::limits::{self, MAX_ARCHIVE_BYTES, MAX_ARCHIVE_DEPTH, MAX_ARCHIVE_NODES};
use crate::{Error, Result};

const LIST_CLASSES: &[&str] = &[
    "NSArray",
    "NSMutableArray",
    "NSSet",
    "NSMutableSet",
    "NSOrderedSet",
    "NSMutableOrderedSet",
];
const DICT_CLASSES: &[&str] = &["NSDictionary", "NSMutableDictionary"];
const STRING_CLASSES: &[&str] = &["NSString", "NSMutableString"];
const DATA_CLASSES: &[&str] = &["NSData", "NSMutableData"];

/// A decoded NSKeyedArchiver value.
#[derive(Debug, Clone)]
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// NSString, or NSUUID formatted as `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`.
    String(String),
    /// NSData, or plain bytes.
    Data(Vec<u8>),
    Date(plist::Date),
    /// NSArray and the other Foundation sequences.
    Array(Vec<Object>),
    /// NSDictionary: key/value pairs in archive order.
    Dict(Vec<(Object, Object)>),
    /// An object of an application class, such as `SilicaLayer`.
    Archived(ArchivedObject),
}

/// An archived object of an application class, with its decoded fields.
#[derive(Debug, Clone, Default)]
pub struct ArchivedObject {
    pub class_name: String,
    /// `$classes` from the archive: the class followed by its superclasses.
    pub class_hierarchy: Vec<String>,
    pub fields: BTreeMap<String, Object>,
}

impl Object {
    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Object::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Object::Int(n) => Some(n),
            Object::Real(f) if f.is_finite() => Some(f as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Object::Int(n) => Some(n as f64),
            Object::Real(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            Object::Data(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_archived(&self) -> Option<&ArchivedObject> {
        match self {
            Object::Archived(a) => Some(a),
            _ => None,
        }
    }

    /// Python-style truth value: null, false, zero, and empty strings or containers are false.
    pub fn truthy(&self) -> bool {
        match self {
            Object::Null => false,
            Object::Bool(b) => *b,
            Object::Int(n) => *n != 0,
            Object::Real(f) => *f != 0.0,
            Object::String(s) => !s.is_empty(),
            Object::Data(d) => !d.is_empty(),
            Object::Array(a) => !a.is_empty(),
            Object::Dict(d) => !d.is_empty(),
            Object::Date(_) | Object::Archived(_) => true,
        }
    }
}

impl ArchivedObject {
    pub fn get(&self, key: &str) -> Option<&Object> {
        self.fields.get(key)
    }

    /// True if the key is present, even when its value is null.
    pub fn has(&self, key: &str) -> bool {
        self.fields.contains_key(key)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Object::as_str)
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Object::as_i64)
    }

    pub fn f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Object::as_f64)
    }

    pub fn truthy(&self, key: &str) -> bool {
        self.get(key).is_some_and(Object::truthy)
    }

    pub fn archived(&self, key: &str) -> Option<&ArchivedObject> {
        self.get(key).and_then(Object::as_archived)
    }

    pub fn array(&self, key: &str) -> Option<&[Object]> {
        self.get(key).and_then(Object::as_array)
    }

    pub fn data(&self, key: &str) -> Option<&[u8]> {
        self.get(key).and_then(Object::as_data)
    }
}

/// A parsed NSKeyedArchiver property list. Objects are decoded on demand.
pub struct KeyedUnarchiver {
    objects: Vec<Value>,
    top: Dictionary,
}

#[derive(Default)]
struct Ctx {
    /// Object indices currently being decoded, for cycle detection.
    stack: Vec<usize>,
    /// Current nesting: referenced objects and inline arrays and dictionaries.
    depth: usize,
    nodes: usize,
    /// String and data bytes produced so far.
    bytes: u64,
}

impl Ctx {
    /// Counts one decoded value holding `bytes` bytes of strings or data.
    fn spend(&mut self, bytes: usize) -> Result<()> {
        self.nodes += 1;
        if self.nodes > MAX_ARCHIVE_NODES {
            return Err(limits::exceeded(format!("the archive expands to more than {MAX_ARCHIVE_NODES} objects")));
        }
        self.bytes += bytes as u64;
        if self.bytes > MAX_ARCHIVE_BYTES {
            return Err(limits::exceeded(format!("the archive expands to more than {MAX_ARCHIVE_BYTES} bytes")));
        }
        Ok(())
    }

    fn enter(&mut self) -> Result<()> {
        if self.depth >= MAX_ARCHIVE_DEPTH {
            return Err(limits::exceeded(format!("the archive nests more than {MAX_ARCHIVE_DEPTH} levels deep")));
        }
        self.depth += 1;
        Ok(())
    }
}

fn uid_index(value: &Value) -> Option<usize> {
    match value {
        Value::Uid(uid) => usize::try_from(uid.get()).ok(),
        _ => None,
    }
}

impl KeyedUnarchiver {
    /// Parses a binary property list and checks that it is an NSKeyedArchiver archive.
    pub fn new(data: &[u8]) -> Result<Self> {
        let Value::Dictionary(mut root) = limits::parse_plist(data)? else {
            return Err(Error::Format("archive root is not a dictionary".into()));
        };
        if root.get("$archiver").and_then(Value::as_string) != Some("NSKeyedArchiver") {
            return Err(Error::Format("not an NSKeyedArchiver archive".into()));
        }
        let objects = match root.remove("$objects") {
            Some(Value::Array(objects)) => objects,
            _ => return Err(Error::Format("$objects is missing or not an array".into())),
        };
        let top = match root.remove("$top") {
            Some(Value::Dictionary(top)) => top,
            _ => return Err(Error::Format("$top is missing or not a dictionary".into())),
        };
        Ok(Self { objects, top })
    }

    /// Decodes `$top["root"]`.
    pub fn root(&self) -> Result<Object> {
        self.top_entry("root")
    }

    /// Decodes the object stored under `$top[key]`.
    pub fn top_entry(&self, key: &str) -> Result<Object> {
        let entry = self
            .top
            .get(key)
            .ok_or_else(|| Error::Format(format!("$top has no entry {key:?}")))?;
        self.decode(entry, &mut Ctx::default())
    }

    fn decode(&self, value: &Value, ctx: &mut Ctx) -> Result<Object> {
        if let Value::Uid(_) = value {
            let idx = uid_index(value).ok_or_else(|| Error::Format("UID out of range".into()))?;
            return self.decode_index(idx, ctx);
        }
        ctx.spend(match value {
            Value::String(s) => s.len(),
            Value::Data(d) => d.len(),
            _ => 0,
        })?;
        if let Value::Array(_) | Value::Dictionary(_) = value {
            ctx.enter()?;
            let result = self.decode_collection(value, ctx);
            ctx.depth -= 1;
            return result;
        }
        Ok(match value {
            Value::Boolean(b) => Object::Bool(*b),
            Value::Integer(_) => match value.as_signed_integer() {
                Some(n) => Object::Int(n),
                None => Object::Real(value.as_unsigned_integer().unwrap_or(0) as f64),
            },
            Value::Real(f) => Object::Real(*f),
            Value::String(s) => Object::String(s.clone()),
            Value::Data(d) => Object::Data(d.clone()),
            Value::Date(d) => Object::Date(*d),
            _ => Object::Null,
        })
    }

    fn decode_collection(&self, value: &Value, ctx: &mut Ctx) -> Result<Object> {
        Ok(match value {
            Value::Array(items) => Object::Array(
                items
                    .iter()
                    .map(|v| self.decode(v, ctx))
                    .collect::<Result<Vec<_>>>()?,
            ),
            Value::Dictionary(d) => Object::Dict(
                d.iter()
                    .map(|(k, v)| {
                        ctx.spend(k.len())?;
                        Ok((Object::String(k.clone()), self.decode(v, ctx)?))
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            _ => Object::Null,
        })
    }

    fn decode_index(&self, idx: usize, ctx: &mut Ctx) -> Result<Object> {
        let raw = self
            .objects
            .get(idx)
            .ok_or_else(|| Error::Format(format!("object index {idx} out of range")))?;
        if matches!(raw, Value::String(s) if s == "$null") {
            return Ok(Object::Null);
        }
        if ctx.stack.contains(&idx) {
            return Ok(Object::Null); // back-reference to an object being decoded
        }
        ctx.enter()?;
        ctx.stack.push(idx);
        let result = match raw {
            Value::Dictionary(dict) if dict.contains_key("$class") => {
                ctx.spend(0).and_then(|()| self.decode_class(dict, ctx))
            }
            other => self.decode(other, ctx),
        };
        ctx.stack.pop();
        ctx.depth -= 1;
        result
    }

    fn field(&self, dict: &Dictionary, key: &str, ctx: &mut Ctx) -> Result<Object> {
        match dict.get(key) {
            Some(v) => self.decode(v, ctx),
            None => Ok(Object::Null),
        }
    }

    fn list(&self, dict: &Dictionary, key: &str, ctx: &mut Ctx) -> Result<Vec<Object>> {
        match dict.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| self.decode(v, ctx))
                .collect::<Result<Vec<_>>>(),
            _ => Ok(Vec::new()),
        }
    }

    fn decode_class(&self, dict: &Dictionary, ctx: &mut Ctx) -> Result<Object> {
        let class_idx = dict
            .get("$class")
            .and_then(uid_index)
            .ok_or_else(|| Error::Format("object has a bad $class reference".into()))?;
        let cls = match self.objects.get(class_idx) {
            Some(Value::Dictionary(cls)) => cls,
            _ => return Err(Error::Format("$class does not refer to a class".into())),
        };
        let name = cls
            .get("$classname")
            .and_then(Value::as_string)
            .unwrap_or("?")
            .to_owned();
        let hierarchy: Vec<String> = match cls.get("$classes").and_then(Value::as_array) {
            Some(list) => list
                .iter()
                .filter_map(Value::as_string)
                .map(str::to_owned)
                .collect(),
            None => vec![name.clone()],
        };
        ctx.spend(hierarchy.iter().map(String::len).sum::<usize>() + name.len())?;
        let name = name.as_str();

        if LIST_CLASSES.contains(&name) {
            return Ok(Object::Array(self.list(dict, "NS.objects", ctx)?));
        }
        if DICT_CLASSES.contains(&name) {
            let keys = self.list(dict, "NS.keys", ctx)?;
            let values = self.list(dict, "NS.objects", ctx)?;
            return Ok(Object::Dict(keys.into_iter().zip(values).collect()));
        }
        if STRING_CLASSES.contains(&name) {
            let text = self.field(dict, "NS.string", ctx)?;
            return Ok(Object::String(text.as_str().unwrap_or("").to_owned()));
        }
        if DATA_CLASSES.contains(&name) {
            let bytes = self.field(dict, "NS.data", ctx)?;
            return Ok(Object::Data(bytes.as_data().unwrap_or(&[]).to_vec()));
        }
        if name == "NSUUID" {
            let bytes = self.field(dict, "NS.uuidbytes", ctx)?;
            let hex: String = bytes
                .as_data()
                .unwrap_or(&[])
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect();
            let text = if hex.len() == 32 {
                format!(
                    "{}-{}-{}-{}-{}",
                    &hex[0..8],
                    &hex[8..12],
                    &hex[12..16],
                    &hex[16..20],
                    &hex[20..32]
                )
            } else {
                hex
            };
            return Ok(Object::String(text));
        }

        let mut fields = BTreeMap::new();
        for (key, value) in dict.iter() {
            if key == "$class" {
                continue;
            }
            ctx.spend(key.len())?;
            fields.insert(key.clone(), self.decode(value, ctx)?);
        }
        Ok(Object::Archived(ArchivedObject {
            class_name: name.to_owned(),
            class_hierarchy: hierarchy,
            fields,
        }))
    }
}

/// Decodes the `root` object of an NSKeyedArchiver archive.
pub fn unarchive(data: &[u8]) -> Result<Object> {
    KeyedUnarchiver::new(data)?.root()
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::Uid;

    fn uid(i: usize) -> Value {
        Value::Uid(Uid::new(i as u64))
    }

    fn class(name: &str) -> Value {
        let mut d = Dictionary::new();
        d.insert("$classname".into(), Value::String(name.into()));
        d.insert(
            "$classes".into(),
            Value::Array(vec![Value::String(name.into()), Value::String("NSObject".into())]),
        );
        Value::Dictionary(d)
    }

    fn archive(objects: Vec<Value>, root: Value) -> Vec<u8> {
        let mut top = Dictionary::new();
        top.insert("root".into(), root);
        let mut doc = Dictionary::new();
        doc.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        doc.insert("$version".into(), Value::Integer(100_000.into()));
        doc.insert("$top".into(), Value::Dictionary(top));
        doc.insert("$objects".into(), Value::Array(objects));
        let mut buf = Vec::new();
        Value::Dictionary(doc).to_writer_binary(&mut buf).unwrap();
        buf
    }

    /// `$objects`: 0 null, 1 SilicaDocument class, 2 document, 3 "x", 4 NSArray, 5 NSArray class.
    fn sample_archive() -> Vec<u8> {
        let mut doc = Dictionary::new();
        doc.insert("$class".into(), uid(1));
        doc.insert("name".into(), uid(3));
        doc.insert("self".into(), uid(2)); // cycle back to the document
        doc.insert("sizes".into(), uid(4));
        doc.insert("tileSize".into(), Value::Integer(256.into()));

        let mut list = Dictionary::new();
        list.insert("$class".into(), uid(5));
        list.insert(
            "NS.objects".into(),
            Value::Array(vec![uid(3), Value::Integer(7.into())]),
        );

        let objects = vec![
            Value::String("$null".into()),
            class("SilicaDocument"),
            Value::Dictionary(doc),
            Value::String("x".into()),
            Value::Dictionary(list),
            class("NSArray"),
        ];
        archive(objects, uid(2))
    }

    #[test]
    fn decodes_archived_objects_and_breaks_cycles() {
        let root = unarchive(&sample_archive()).unwrap();
        let doc = root.as_archived().expect("archived root");
        assert_eq!(doc.class_name, "SilicaDocument");
        assert_eq!(doc.class_hierarchy, ["SilicaDocument", "NSObject"]);
        assert_eq!(doc.str("name"), Some("x"));
        assert!(doc.get("self").unwrap().is_null());
        assert_eq!(doc.i64("tileSize"), Some(256));
        let sizes = doc.array("sizes").expect("array");
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes[0].as_str(), Some("x"));
        assert_eq!(sizes[1].as_i64(), Some(7));
    }

    #[test]
    fn rejects_non_keyed_archives() {
        let mut plain = Dictionary::new();
        plain.insert("name".into(), Value::String("x".into()));
        let mut buf = Vec::new();
        Value::Dictionary(plain).to_writer_binary(&mut buf).unwrap();
        assert!(matches!(unarchive(&buf), Err(Error::Format(_))));
        assert!(matches!(unarchive(b"not a plist"), Err(Error::Plist(_))));
    }

    #[test]
    fn rejects_out_of_range_references() {
        let objects = vec![Value::String("$null".into()), class("SilicaDocument")];
        let data = archive(objects, uid(42));
        assert!(matches!(unarchive(&data), Err(Error::Format(_))));
    }
}

// ------------------------------------------------------------------ NSKeyedArchiver writer

/// A Foundation number, archived as an `NSNumber` object.
#[derive(Debug, Clone, PartialEq)]
pub enum NsNumber {
    Int(i64),
    Real(f64),
    Bool(bool),
}

/// A value to archive.
///
/// Booleans, integers and reals are stored inline, as `encodeBool:`, `encodeInt64:` and
/// `encodeDouble:` do. Strings, data, numbers, collections and objects each get an `$objects`
/// entry and are referenced by UID. [`NsValue::Null`] refers to the `$null` entry.
#[derive(Debug, Clone, PartialEq)]
pub enum NsValue {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// `NSString`, stored as a bare string in `$objects`.
    String(String),
    /// `NSData`.
    Data(Vec<u8>),
    /// `NSNumber`, with `NS.intval`, `NS.dblval` or `NS.boolval`.
    Number(NsNumber),
    /// `NSArray`, or `NSMutableArray` when `mutable` is set.
    Array { items: Vec<NsValue>, mutable: bool },
    /// `NSDictionary`, or `NSMutableDictionary` when `mutable` is set. Keys are strings.
    Dict { entries: Vec<(String, NsValue)>, mutable: bool },
    /// An object of an application class, such as `SilicaLayer`.
    Object(NsObject),
}

/// An archived object of an application class.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NsObject {
    pub class: String,
    /// Superclasses listed after `class` in `$classes`. Empty means `["NSObject"]`.
    pub superclasses: Vec<String>,
    /// Fields in archive order, keyed by their coder key.
    pub fields: Vec<(String, NsValue)>,
}

impl NsObject {
    /// An object of `class` whose superclass chain is `NSObject`.
    pub fn new(class: impl Into<String>) -> Self {
        Self {
            class: class.into(),
            superclasses: vec!["NSObject".into()],
            fields: Vec::new(),
        }
    }

    /// Adds a field (builder style).
    pub fn field(mut self, key: impl Into<String>, value: NsValue) -> Self {
        self.fields.push((key.into(), value));
        self
    }
}

/// State while building `$objects`. Slot 0 is always `$null`.
struct Encoder {
    objects: Vec<Value>,
    /// Class chains (class followed by its superclasses) and their class-dictionary slots.
    classes: Vec<(Vec<String>, usize)>,
}

impl Encoder {
    fn push(&mut self, value: Value) -> usize {
        self.objects.push(value);
        self.objects.len() - 1
    }

    fn uid(index: usize) -> Value {
        Value::Uid(plist::Uid::new(index as u64))
    }

    /// The UID of the class dictionary for `class`, creating it on first use.
    fn class_ref<S: AsRef<str>>(&mut self, class: &str, supers: &[S]) -> Value {
        let mut chain = vec![class.to_owned()];
        chain.extend(supers.iter().map(|s| s.as_ref().to_owned()));
        if let Some((_, idx)) = self.classes.iter().find(|(c, _)| *c == chain) {
            return Self::uid(*idx);
        }
        let mut d = Dictionary::new();
        d.insert("$classname".into(), Value::String(class.to_owned()));
        d.insert(
            "$classes".into(),
            Value::Array(chain.iter().map(|c| Value::String(c.clone())).collect()),
        );
        let idx = self.push(Value::Dictionary(d));
        self.classes.push((chain, idx));
        Self::uid(idx)
    }

    /// Encodes `value`, returning the inline value or the UID of its `$objects` slot.
    fn encode(&mut self, value: &NsValue) -> Value {
        match value {
            NsValue::Null => Self::uid(0),
            NsValue::Bool(b) => Value::Boolean(*b),
            NsValue::Int(n) => Value::Integer((*n).into()),
            NsValue::Real(f) => Value::Real(*f),
            NsValue::String(s) => Self::uid(self.push(Value::String(s.clone()))),
            NsValue::Data(bytes) => {
                let class = self.class_ref("NSData", &["NSObject"]);
                let mut d = Dictionary::new();
                d.insert("NS.data".into(), Value::Data(bytes.clone()));
                d.insert("$class".into(), class);
                Self::uid(self.push(Value::Dictionary(d)))
            }
            NsValue::Number(number) => {
                let class = self.class_ref("NSNumber", &["NSValue", "NSObject"]);
                let mut d = Dictionary::new();
                match number {
                    NsNumber::Int(v) => d.insert("NS.intval".into(), Value::Integer((*v).into())),
                    NsNumber::Real(v) => d.insert("NS.dblval".into(), Value::Real(*v)),
                    NsNumber::Bool(v) => d.insert("NS.boolval".into(), Value::Boolean(*v)),
                };
                d.insert("$class".into(), class);
                Self::uid(self.push(Value::Dictionary(d)))
            }
            NsValue::Array { items, mutable } => {
                let idx = self.push(Value::Boolean(false));
                let encoded: Vec<Value> = items.iter().map(|item| self.encode(item)).collect();
                let class = if *mutable {
                    self.class_ref("NSMutableArray", &["NSArray", "NSObject"])
                } else {
                    self.class_ref("NSArray", &["NSObject"])
                };
                let mut d = Dictionary::new();
                d.insert("NS.objects".into(), Value::Array(encoded));
                d.insert("$class".into(), class);
                self.objects[idx] = Value::Dictionary(d);
                Self::uid(idx)
            }
            NsValue::Dict { entries, mutable } => {
                let idx = self.push(Value::Boolean(false));
                let mut keys = Vec::with_capacity(entries.len());
                let mut values = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    keys.push(self.encode(&NsValue::String(key.clone())));
                    values.push(self.encode(value));
                }
                let class = if *mutable {
                    self.class_ref("NSMutableDictionary", &["NSDictionary", "NSObject"])
                } else {
                    self.class_ref("NSDictionary", &["NSObject"])
                };
                let mut d = Dictionary::new();
                d.insert("NS.keys".into(), Value::Array(keys));
                d.insert("NS.objects".into(), Value::Array(values));
                d.insert("$class".into(), class);
                self.objects[idx] = Value::Dictionary(d);
                Self::uid(idx)
            }
            NsValue::Object(object) => {
                let idx = self.push(Value::Boolean(false));
                let mut d = Dictionary::new();
                for (key, field) in &object.fields {
                    let encoded = self.encode(field);
                    d.insert(key.clone(), encoded);
                }
                let supers: Vec<String> = if object.superclasses.is_empty() {
                    vec!["NSObject".into()]
                } else {
                    object.superclasses.clone()
                };
                let class = self.class_ref(&object.class, &supers[..]);
                d.insert("$class".into(), class);
                self.objects[idx] = Value::Dictionary(d);
                Self::uid(idx)
            }
        }
    }
}

/// Serialises `root` as an NSKeyedArchiver archive in binary property list (`bplist00`) form.
///
/// The result has `$archiver`, `$version` (100000), `$top` with the key `root`, and `$objects`.
/// Objects are emitted in encounter order and shared class dictionaries are emitted once.
pub fn write(root: &NsValue) -> Result<Vec<u8>> {
    let mut enc = Encoder {
        objects: vec![Value::String("$null".into())],
        classes: Vec::new(),
    };
    let top = match enc.encode(root) {
        uid @ Value::Uid(_) => uid,
        inline => Encoder::uid(enc.push(inline)),
    };
    let mut top_dict = Dictionary::new();
    top_dict.insert("root".into(), top);
    let mut doc = Dictionary::new();
    doc.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
    doc.insert("$version".into(), Value::Integer(100_000.into()));
    doc.insert("$top".into(), Value::Dictionary(top_dict));
    doc.insert("$objects".into(), Value::Array(enc.objects));
    let mut buf = Vec::new();
    Value::Dictionary(doc).to_writer_binary(&mut buf)?;
    Ok(buf)
}
