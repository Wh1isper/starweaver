//! Error registration for the native module.

// Python-facing exception classes live in the pure Python package so they can
// carry ergonomic attributes while the Rust tool bridge maps by stable class
// names.
