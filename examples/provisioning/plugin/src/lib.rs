#![no_std]

extern crate alloc;

use alloc::{format, string::String, vec, vec::Vec};
use lockgate_plugin::{MetadataSource, Need, Needs, NoSettings, ScopeRef};
use provisioning_policy::permissions::vm;

lockgate_plugin::generate!({
    path: "../wit",
    world: "plugin",
});

const REQUIRED: &[Need] = &[
    vm::CREATE.need(&[ScopeRef::literal("gpu")]),
    vm::EXEC.need(&[
        ScopeRef::literal("pool:gpu"),
        ScopeRef::literal("created-by-caller"),
    ]),
    vm::DESTROY.need(&[ScopeRef::literal("created-by-caller")]),
];

struct Provisioner;

impl lockgate_plugin::Plugin for Provisioner {
    const ID: &'static str = "provisioning";
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::required(REQUIRED);
    type Settings = NoSettings;
}

impl exports::example::provisioning::provisioner::Guest for Provisioner {
    fn run() -> Result<Vec<String>, String> {
        let protocol = example::provisioning::vm::protocol_version();
        let created = example::provisioning::vm::create("gpu")
            .map_err(|error| format!("VM create failed: {error:?}"))?;
        example::provisioning::vm::exec(&created, "run isolated job")
            .map_err(|error| format!("created VM exec failed: {error:?}"))?;
        example::provisioning::vm::exec("gpu/base", "inspect accelerator")
            .map_err(|error| format!("gpu pool exec failed: {error:?}"))?;

        let denied = match example::provisioning::vm::exec("cpu/base", "read secrets") {
            Err(example::provisioning::vm::VmError::Denied(atom)) => atom,
            Err(error) => return Err(format!("unexpected cpu VM error: {error:?}")),
            Ok(_) => return Err("cpu VM exec unexpectedly succeeded".into()),
        };

        example::provisioning::vm::destroy(&created)
            .map_err(|error| format!("created VM destroy failed: {error:?}"))?;

        Ok(vec![
            format!("protocol version: {protocol}"),
            format!("allowed: vm.create pool=gpu -> {created}"),
            format!("allowed: vm.exec vm={created} via=created-by-caller"),
            "allowed: vm.exec vm=gpu/base via=pool:gpu".into(),
            format!("denied: vm.exec vm=cpu/base atom={denied}"),
            format!("allowed: vm.destroy vm={created} via=created-by-caller"),
            "plugin outcome: explicit create/exec/destroy grants enforced by membership".into(),
        ])
    }
}

lockgate_plugin::export!(Provisioner);
