//! The application's services: business logic over the models.
//!
//! A service is one file and framework-agnostic (takes `&Db`, returns domain
//! values). The controllers map these results to HTTP responses.

pub mod user_service;
