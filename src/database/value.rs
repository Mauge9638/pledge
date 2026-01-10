use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PostcardValue {
    Object(Vec<(String, PostcardValue)>), // Vec instead of HashMap as it's more efficient for small data sets and Postcard gets mad at HashMap
    Array(Vec<PostcardValue>),
    String(String),
    Integer8(i8),
    Integer16(i16),
    Integer32(i32),
    Integer64(i64),
    Float32(f32),
    Float64(f64),
    Bool(bool),
    Null,
}

// Manual Serialize so JSON looks normal
// impl Serialize for PostcardValue {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         match self {
//             PostcardValue::Object(fields) => {
//                 use serde::ser::SerializeMap;
//                 let mut map = serializer.serialize_map(Some(fields.len()))?;
//                 for (k, v) in fields {
//                     map.serialize_entry(k, v)?;
//                 }
//                 map.end()
//             }
//             PostcardValue::Array(arr) => {
//                 use serde::ser::SerializeSeq;
//                 let mut seq = serializer.serialize_seq(Some(arr.len()))?;
//                 for item in arr {
//                     seq.serialize_element(item)?;
//                 }
//                 seq.end()
//             }
//             PostcardValue::String(s) => serializer.serialize_str(s),
//             PostcardValue::Integer8(i) => serializer.serialize_i8(*i),
//             PostcardValue::Integer16(i) => serializer.serialize_i16(*i),
//             PostcardValue::Integer32(i) => serializer.serialize_i32(*i),
//             PostcardValue::Integer64(i) => serializer.serialize_i64(*i),
//             PostcardValue::Float32(f) => serializer.serialize_f32(*f),
//             PostcardValue::Float64(f) => serializer.serialize_f64(*f),
//             PostcardValue::Bool(b) => serializer.serialize_bool(*b),
//             PostcardValue::Null => serializer.serialize_unit(),
//         }
//     }
// }

// impl<'de> Deserialize<'de> for PostcardValue {
//     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
//     where
//         D: Deserializer<'de>,
//     {
//         use serde::de::{MapAccess, SeqAccess, Visitor};

//         struct ValueVisitor;

//         impl<'de> Visitor<'de> for ValueVisitor {
//             type Value = PostcardValue;

//             fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
//                 formatter.write_str("any valid PostcardValue")
//             }

//             fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Bool(v))
//             }

//             fn visit_i8<E>(self, v: i8) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer8(v))
//             }

//             fn visit_i16<E>(self, v: i16) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer16(v))
//             }

//             fn visit_i32<E>(self, v: i32) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer32(v))
//             }

//             fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer64(v))
//             }

//             fn visit_u8<E>(self, v: u8) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer16(v as i16))
//             }

//             fn visit_u16<E>(self, v: u16) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer32(v as i32))
//             }

//             fn visit_u32<E>(self, v: u32) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer64(v as i64))
//             }

//             fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Integer64(v as i64))
//             }

//             fn visit_f32<E>(self, v: f32) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Float32(v))
//             }

//             fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Float64(v))
//             }

//             fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::String(v.to_string()))
//             }

//             fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::String(v))
//             }

//             fn visit_unit<E>(self) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Null)
//             }

//             fn visit_none<E>(self) -> Result<Self::Value, E> {
//                 Ok(PostcardValue::Null)
//             }

//             fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
//             where
//                 A: SeqAccess<'de>,
//             {
//                 let mut vec = Vec::new();
//                 while let Some(elem) = seq.next_element()? {
//                     vec.push(elem);
//                 }
//                 Ok(PostcardValue::Array(vec))
//             }

//             fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
//             where
//                 A: MapAccess<'de>,
//             {
//                 let mut fields = Vec::new();
//                 while let Some((key, value)) = map.next_entry()? {
//                     fields.push((key, value));
//                 }
//                 Ok(PostcardValue::Object(fields))
//             }
//         }

//         deserializer.deserialize_any(ValueVisitor)
//     }
// }
