use crate::storage::{admin, pause, pending_admin, relayers};
use soroban_sdk::{Address, Env};

pub fn require_admin(env: &Env, caller: &Address) {
    caller.require_auth();
    if *caller != admin::get(env) {
        panic!("not admin")
    }
}

pub fn require_relayer(env: &Env, caller: &Address) {
    caller.require_auth();
    if !relayers::has(env, caller) {
        panic!("not relayer")
    }
}

pub fn require_admin_or_relayer(env: &Env, caller: &Address) {
    caller.require_auth();
    if *caller != admin::get(env) && !relayers::has(env, caller) {
        panic!("not admin or relayer")
    }
}

pub fn require_not_paused(env: &Env) {
    if pause::is_paused(env) {
        panic!("contract paused")
    }
}

/// @notice Proposes a new admin. Must be called by the current admin.
/// @dev Stores candidate in pending_admin; does not transfer admin rights yet.
pub fn set_pending_admin(env: &Env, caller: &Address, candidate: &Address) {
    require_admin(env, caller);
    pending_admin::set(env, candidate);
}

/// @notice Completes the two-step admin transfer. Must be called by the pending admin.
/// @dev Clears pending_admin after promotion to prevent replay.
pub fn accept_pending_admin(env: &Env, caller: &Address) {
    caller.require_auth();
    let candidate = pending_admin::get(env).expect("no pending admin");
    if *caller != candidate {
        panic!("not pending admin")
    }
    admin::set(env, caller);
    pending_admin::clear(env);
}
