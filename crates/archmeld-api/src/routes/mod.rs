// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! HTTP handlers grouped by resource domain. The router and shared
//! authentication/error plumbing stay in `lib.rs`.

pub(crate) mod architecture;
pub(crate) mod changesets;
pub(crate) mod interpret;
pub(crate) mod jobs;
pub(crate) mod ops;
pub(crate) mod projects;
pub(crate) mod query;
pub(crate) mod runtime;
pub(crate) mod scenarios;
pub(crate) mod status_review;

pub(crate) use architecture::*;
pub(crate) use changesets::*;
pub(crate) use interpret::*;
pub(crate) use jobs::*;
pub(crate) use ops::*;
pub(crate) use projects::*;
pub(crate) use query::*;
pub(crate) use runtime::*;
pub(crate) use scenarios::*;
pub(crate) use status_review::*;
