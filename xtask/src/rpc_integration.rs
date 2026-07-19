use crate::{
    common::root,
    rpc_contracts,
    rpc_interop_e2e::{self, build_binaries},
};

pub fn check(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("check-rpc-integration takes no arguments".to_string());
    }
    let repository = root()?;
    let (cli, rpc) = build_binaries(&repository)?;
    rpc_contracts::check_transports_with_rpc(&rpc)?;
    rpc_interop_e2e::check_with_binaries(&cli, &rpc)
}
