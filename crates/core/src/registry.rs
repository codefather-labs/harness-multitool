//! IF-0.3: resolves `(interface, name)` from the configuration to an implementation linked
//! into the build. An unknown name is a startup error naming what is known.

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use harness_api::platform::Platform;
use harness_api::{Auth, ContextStrategy, Frontend, Hook, Memory, Policy, Provider, Router, SessionStore, Tool, Verifier};
use serde_json::Value;

/// Which implementation of each interface a session uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub auth: String,
    pub providers: Vec<String>,
    pub router: String,
    pub tools: Vec<String>,
    pub policy: String,
    pub hooks: Vec<String>,
    pub memory: String,
    pub context: String,
    pub store: String,
    pub verifier: String,
}

/// What a factory gets. `settings` holds the free-form settings of this implementation.
pub struct BuildContext<'a> {
    pub platform: &'a Platform,
    /// `None` only while the `Auth` implementation itself is being built.
    pub auth: Option<Rc<dyn Auth>>,
    /// `None` until the `Memory` implementation is built; tools over it see it.
    pub memory: Option<Rc<dyn Memory>>,
    pub settings: &'a Value,
}

type Factory<T> = Box<dyn Fn(&BuildContext<'_>) -> Result<Rc<T>, String>>;

/// One interface's implementations by name.
struct Slot<T: ?Sized> {
    interface: &'static str,
    known: BTreeMap<String, Factory<T>>,
}

impl<T: ?Sized> Slot<T> {
    fn new(interface: &'static str) -> Self {
        Slot { interface, known: BTreeMap::new() }
    }

    fn build(&self, name: &str, cx: &BuildContext<'_>) -> Result<Rc<T>, RegistryError> {
        let factory = self.known.get(name).ok_or_else(|| RegistryError::Unknown {
            interface: self.interface,
            name: name.to_string(),
            known: self.known.keys().cloned().collect(),
        })?;
        factory(cx).map_err(|message| RegistryError::Build { interface: self.interface, name: name.to_string(), message })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    Unknown { interface: &'static str, name: String, known: Vec<String> },
    Build { interface: &'static str, name: String, message: String },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Unknown { interface, name, known } => {
                write!(f, "no {interface} named \"{name}\" in this build; known: {}", known.join(", "))
            }
            RegistryError::Build { interface, name, message } => write!(f, "{interface} \"{name}\" failed to start: {message}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// The resolved components of a session.
pub struct Parts {
    /// Runs turns instead of the providers, when set (IF-17).
    pub backend: Option<Rc<dyn harness_api::AgentBackend>>,
    /// What the harness left running for this session, if it runs its own turns.
    pub background: Option<Rc<dyn harness_api::BackgroundWatch>>,
    /// The backend's own form of a conversation, to hand it a session made elsewhere (IF-18).
    pub conversations: Option<Rc<dyn harness_api::ConversationFormat>>,
    pub providers: BTreeMap<String, Rc<dyn Provider>>,
    pub router: Rc<dyn Router>,
    pub tools: BTreeMap<String, Rc<dyn Tool>>,
    pub policy: Rc<dyn Policy>,
    pub hooks: Vec<Rc<dyn Hook>>,
    pub memory: Rc<dyn Memory>,
    pub context: Rc<dyn ContextStrategy>,
    pub store: Rc<dyn SessionStore>,
    pub verifier: Rc<dyn Verifier>,
    pub platform: Platform,
}

pub struct Registry {
    auth: Slot<dyn Auth>,
    providers: Slot<dyn Provider>,
    routers: Slot<dyn Router>,
    tools: Slot<dyn Tool>,
    policies: Slot<dyn Policy>,
    hooks: Slot<dyn Hook>,
    memories: Slot<dyn Memory>,
    contexts: Slot<dyn ContextStrategy>,
    stores: Slot<dyn SessionStore>,
    verifiers: Slot<dyn Verifier>,
    settings: BTreeMap<String, Value>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            auth: Slot::new("Auth"),
            providers: Slot::new("Provider"),
            routers: Slot::new("Router"),
            tools: Slot::new("Tool"),
            policies: Slot::new("Policy"),
            hooks: Slot::new("Hook"),
            memories: Slot::new("Memory"),
            contexts: Slot::new("ContextStrategy"),
            stores: Slot::new("SessionStore"),
            verifiers: Slot::new("Verifier"),
            settings: BTreeMap::new(),
        }
    }
}

macro_rules! register {
    ($fn_name:ident, $slot:ident, $trait:ty) => {
        pub fn $fn_name(&mut self, name: &str, factory: impl Fn(&BuildContext<'_>) -> Result<Rc<$trait>, String> + 'static) -> &mut Self {
            self.$slot.known.insert(name.to_string(), Box::new(factory));
            self
        }
    };
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    register!(auth, auth, dyn Auth);
    register!(provider, providers, dyn Provider);
    register!(router, routers, dyn Router);
    register!(tool, tools, dyn Tool);
    register!(policy, policies, dyn Policy);
    register!(hook, hooks, dyn Hook);
    register!(memory, memories, dyn Memory);
    register!(context, contexts, dyn ContextStrategy);
    register!(store, stores, dyn SessionStore);
    register!(verifier, verifiers, dyn Verifier);

    /// The settings of the implementation `name` of `interface`, e.g. `("provider", "anthropic")`.
    pub fn settings(&mut self, interface: &str, name: &str, value: Value) -> &mut Self {
        self.settings.insert(format!("{interface}.{name}"), value);
        self
    }

    pub fn resolve(&self, selection: &Selection, platform: Platform) -> Result<Parts, RegistryError> {
        let none = Value::Null;
        let auth_cx = BuildContext { platform: &platform, auth: None, memory: None, settings: self.settings.get(&format!("auth.{}", selection.auth)).unwrap_or(&none) };
        let auth = self.auth.build(&selection.auth, &auth_cx)?;
        let memory_cx = BuildContext { platform: &platform, auth: Some(auth.clone()), memory: None, settings: self.settings.get(&format!("memory.{}", selection.memory)).unwrap_or(&none) };
        let memory = self.memories.build(&selection.memory, &memory_cx)?;
        let tools_memory = memory.clone();
        let cx = |interface: &str, name: &str| BuildContext {
            platform: &platform,
            auth: Some(auth.clone()),
            memory: Some(tools_memory.clone()),
            settings: self.settings.get(&format!("{interface}.{name}")).unwrap_or(&none),
        };

        let mut providers = BTreeMap::new();
        for name in &selection.providers {
            providers.insert(name.clone(), self.providers.build(name, &cx("provider", name))?);
        }
        let mut tools = BTreeMap::new();
        for name in &selection.tools {
            let tool = self.tools.build(name, &cx("tool", name))?;
            tools.insert(tool.spec().name, tool);
        }
        let mut hooks = Vec::new();
        for name in &selection.hooks {
            hooks.push(self.hooks.build(name, &cx("hook", name))?);
        }
        Ok(Parts {
            background: None,
            backend: None,
            conversations: None,
            providers,
            router: self.routers.build(&selection.router, &cx("router", &selection.router))?,
            tools,
            policy: self.policies.build(&selection.policy, &cx("policy", &selection.policy))?,
            hooks,
            memory,
            context: self.contexts.build(&selection.context, &cx("context", &selection.context))?,
            store: self.stores.build(&selection.store, &cx("store", &selection.store))?,
            verifier: self.verifiers.build(&selection.verifier, &cx("verifier", &selection.verifier))?,
            platform,
        })
    }
}

/// A frontend is not selected by name here: the running binary is the frontend and passes it
/// to the engine.
pub type SharedFrontend = Rc<dyn Frontend>;
