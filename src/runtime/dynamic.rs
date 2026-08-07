//! Optional runtime-typed registry host implementation.
//! Exact lookup grants become bounded per-store handles before values cross into another store.

use super::{Event, PluginStore, emit, invoke_target};
use crate::{lockgate::core::registry, plan::Target};
use wasmtime::component::{Val, types::Type};

impl<H: Send + 'static> registry::Host for PluginStore<H> {
    fn lookup(&mut self, target: String) -> Result<u32, registry::InvokeError> {
        let Some(resolved) = self.lookups.get(&target).cloned() else {
            emit(
                &self.runtimes,
                Event::DynamicLookup {
                    caller: self.component_name.clone(),
                    target,
                    allowed: false,
                },
            );
            return Err(registry::InvokeError::Denied("not permitted".into()));
        };
        if let Some(handle) = self.target_handles.get(&target) {
            emit(
                &self.runtimes,
                Event::DynamicLookup {
                    caller: self.component_name.clone(),
                    target,
                    allowed: true,
                },
            );
            return Ok(*handle);
        }
        let handle = u32::try_from(self.handles.len() + 1)
            .map_err(|_| registry::InvokeError::Trapped("handle table exhausted".into()))?;
        emit(
            &self.runtimes,
            Event::DynamicLookup {
                caller: self.component_name.clone(),
                target: target.clone(),
                allowed: true,
            },
        );
        self.handles.insert(handle, resolved);
        self.target_handles.insert(target, handle);
        Ok(handle)
    }

    fn invoke(
        &mut self,
        handle: u32,
        args: Vec<registry::Value>,
    ) -> Result<Vec<registry::Value>, registry::InvokeError> {
        let Some(target) = self.handles.get(&handle).cloned() else {
            return Err(registry::InvokeError::Denied(format!(
                "unknown handle {handle}"
            )));
        };
        check_values(&target, &args)?;
        let Some(runtimes) = self.runtimes.upgrade() else {
            return Err(registry::InvokeError::Trapped("runtime unavailable".into()));
        };
        let params = args.into_iter().map(value_to_val).collect::<Vec<_>>();
        invoke_target(&runtimes, &target, &params)
            .map_err(|error| registry::InvokeError::Trapped(error.to_string()))?
            .into_iter()
            .map(val_to_value)
            .collect()
    }
}

fn check_values(target: &Target, args: &[registry::Value]) -> Result<(), registry::InvokeError> {
    if args.len() != target.signature.params.len() {
        return Err(registry::InvokeError::TypeMismatch(format!(
            "expected {} arguments, got {}",
            target.signature.params.len(),
            args.len()
        )));
    }
    for (index, (value, expected)) in args.iter().zip(&target.signature.params).enumerate() {
        if !dynamic_value_matches(value, expected) {
            return Err(registry::InvokeError::TypeMismatch(format!(
                "arg {index} expected {}, got {}",
                crate::plugin::type_name(expected),
                value_name(value)
            )));
        }
    }
    Ok(())
}

fn dynamic_value_matches(value: &registry::Value, expected: &Type) -> bool {
    match (value, expected) {
        (registry::Value::Bool(_), Type::Bool)
        | (registry::Value::S32(_), Type::S32)
        | (registry::Value::U32(_), Type::U32)
        | (registry::Value::String(_), Type::String) => true,
        (registry::Value::List(_), Type::List(list)) => list.ty() == Type::String,
        _ => false,
    }
}

fn value_name(value: &registry::Value) -> &str {
    match value {
        registry::Value::Bool(_) => "bool",
        registry::Value::S32(_) => "s32",
        registry::Value::U32(_) => "u32",
        registry::Value::String(_) => "string",
        registry::Value::List(_) => "list<string>",
    }
}

fn value_to_val(value: registry::Value) -> Val {
    match value {
        registry::Value::Bool(value) => Val::Bool(value),
        registry::Value::S32(value) => Val::S32(value),
        registry::Value::U32(value) => Val::U32(value),
        registry::Value::String(value) => Val::String(value),
        registry::Value::List(values) => Val::List(values.into_iter().map(Val::String).collect()),
    }
}

fn val_to_value(value: Val) -> Result<registry::Value, registry::InvokeError> {
    match value {
        Val::Bool(value) => Ok(registry::Value::Bool(value)),
        Val::S32(value) => Ok(registry::Value::S32(value)),
        Val::U32(value) => Ok(registry::Value::U32(value)),
        Val::String(value) => Ok(registry::Value::String(value)),
        Val::List(values) => values
            .into_iter()
            .map(|value| match value {
                Val::String(string) => Ok(string),
                other => Err(registry::InvokeError::Trapped(format!(
                    "unsupported list item {other:?}"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(registry::Value::List),
        other => Err(registry::InvokeError::Trapped(format!(
            "unsupported result {other:?}"
        ))),
    }
}
