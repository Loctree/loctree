//! Minimal SCIP protobuf bindings used by the decode-only deep index importer.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
//! This mirrors the subset of `scip.proto` needed to consume
//! `sourcegraph/scip-clang` indexes: `Index`, `Document`, `Occurrence`,
//! `SymbolInformation`, and `Relationship`. Unknown fields are intentionally
//! ignored by prost so newer SCIP producers remain forward-compatible.

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Index {
    #[prost(message, repeated, tag = "2")]
    pub documents: Vec<Document>,
    #[prost(message, repeated, tag = "3")]
    pub external_symbols: Vec<SymbolInformation>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Document {
    #[prost(string, tag = "1")]
    pub relative_path: String,
    #[prost(message, repeated, tag = "2")]
    pub occurrences: Vec<Occurrence>,
    #[prost(message, repeated, tag = "3")]
    pub symbols: Vec<SymbolInformation>,
    #[prost(string, tag = "4")]
    pub language: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Occurrence {
    #[prost(int32, repeated, tag = "1")]
    pub range: Vec<i32>,
    #[prost(string, tag = "2")]
    pub symbol: String,
    #[prost(int32, tag = "3")]
    pub symbol_roles: i32,
    #[prost(message, repeated, tag = "5")]
    pub relationships: Vec<Relationship>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SymbolInformation {
    #[prost(string, tag = "1")]
    pub symbol: String,
    #[prost(message, repeated, tag = "4")]
    pub relationships: Vec<Relationship>,
    #[prost(string, tag = "5")]
    pub display_name: String,
    #[prost(string, tag = "6")]
    pub signature_documentation: String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Relationship {
    #[prost(string, tag = "1")]
    pub symbol: String,
    #[prost(bool, tag = "2")]
    pub is_reference: bool,
    #[prost(bool, tag = "3")]
    pub is_implementation: bool,
    #[prost(bool, tag = "4")]
    pub is_type_definition: bool,
    #[prost(bool, tag = "5")]
    pub is_definition: bool,
}
