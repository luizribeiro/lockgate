#![no_std]

//! Authority contract shared by the provisioning host and plugin.

extern crate alloc;

pub mod permissions {
    #[lockgate_policy::capability("vm")]
    pub mod vm {
        use alloc::string::{String, ToString};
        use core::str::FromStr;
        use lockgate_policy::{Permission, Scope, ScopeError, ScopeRepr, ScopedPermission};

        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum PoolScope {
            Any,
            Named(String),
        }

        impl FromStr for PoolScope {
            type Err = ScopeError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    "*" => Ok(Self::Any),
                    "" => Err(ScopeError::unknown(value)),
                    name => Ok(Self::Named(name.to_string())),
                }
            }
        }

        impl ScopeRepr for PoolScope {
            fn canonical(&self) -> String {
                match self {
                    Self::Any => "*".to_string(),
                    Self::Named(name) => name.clone(),
                }
            }
        }

        impl Scope for PoolScope {
            fn contains(&self, inner: &Self) -> bool {
                self == inner || matches!(self, Self::Any)
            }
        }

        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum InstanceScope {
            Any,
            Pool(String),
            CreatedByCaller,
        }

        impl FromStr for InstanceScope {
            type Err = ScopeError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    "*" => Ok(Self::Any),
                    "created-by-caller" => Ok(Self::CreatedByCaller),
                    value => match value.strip_prefix("pool:") {
                        Some("") | None => Err(ScopeError::unknown(value)),
                        Some(pool) => Ok(Self::Pool(pool.to_string())),
                    },
                }
            }
        }

        impl ScopeRepr for InstanceScope {
            fn canonical(&self) -> String {
                match self {
                    Self::Any => "*".to_string(),
                    Self::Pool(pool) => alloc::format!("pool:{pool}"),
                    Self::CreatedByCaller => "created-by-caller".to_string(),
                }
            }
        }

        impl Scope for InstanceScope {
            fn contains(&self, inner: &Self) -> bool {
                self == inner || matches!(self, Self::Any)
            }
        }

        /// Create a virtual machine in a pool.
        pub const CREATE: ScopedPermission<PoolScope> = ScopedPermission::new("create");

        /// Run a command on a virtual machine.
        pub const EXEC: ScopedPermission<InstanceScope> = ScopedPermission::new("exec");

        /// Delete a virtual machine.
        pub const DESTROY: ScopedPermission<InstanceScope> = ScopedPermission::new("destroy");

        /// Discover available pools.
        pub const LIST_POOLS: Permission = Permission::new("list-pools");
    }
}

#[cfg(test)]
mod tests {
    use lockgate_policy::check_scope_laws;

    use super::permissions::vm::{InstanceScope, PoolScope};

    #[test]
    fn vm_scope_samples_obey_the_scope_laws() {
        check_scope_laws([
            PoolScope::Any,
            PoolScope::Named("gpu".into()),
            PoolScope::Named("cpu".into()),
        ])
        .unwrap();
        check_scope_laws([
            InstanceScope::Any,
            InstanceScope::Pool("gpu".into()),
            InstanceScope::Pool("cpu".into()),
            InstanceScope::CreatedByCaller,
        ])
        .unwrap();
    }
}
