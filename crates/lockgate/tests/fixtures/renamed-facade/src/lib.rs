#![no_std]

macro_rules! __lockgate_wit_export {
    ($plugin:ident) => {};
}

struct RenamedFacade;

impl lgp::Plugin for RenamedFacade {
    const ID: &'static str = "renamed-facade";
}

lgp::export!(RenamedFacade);
