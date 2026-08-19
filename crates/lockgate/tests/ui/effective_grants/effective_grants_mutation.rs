use lockgate::EffectiveGrants;

fn mutate(grants: &mut EffectiveGrants) {
    grants.insert_flag("sessions.send");
}

fn main() {}
