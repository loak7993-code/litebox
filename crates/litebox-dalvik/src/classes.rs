//! Class registry: interning (types, names, shorties, strings), dex class
//! loading, instance layouts, dispatch maps, subtype checks, static storage,
//! and the native method table.
//!
//! Resolution model:
//! - decode interns pool references into u32 ids; structural resolution
//!   (direct targets, field slots) is lazy with caches keyed by dex position,
//!   so cross-class references resolve correctly regardless of load order.
//! - instance field slots are inherited-first; a field's slot in its
//!   declaring class equals its slot in every subclass (subclasses append).
//! - shim classes (framework) finalize lazily: super chain load, map merge,
//!   native method materialization.

use crate::decode::decode_method;
use crate::decode_types::MethodCode;
use crate::errors::{ThrowSpec, VmFault};
use crate::natives::NativeRegistry;
use crate::value::{PrimKind, Value};
use litebox_dex::Dex;
use std::collections::HashMap;
use std::sync::Arc;

pub const ACC_STATIC: u32 = 0x0008;
pub const ACC_NATIVE: u32 = 0x0100;
pub const ACC_ABSTRACT: u32 = 0x0400;
pub const ACC_INTERFACE: u32 = 0x0200;
pub const ACC_ENUM: u32 = 0x4000;

#[derive(Debug, Clone)]
pub enum ClassKind {
    Dex { dex: u32, def: u32 },
    Shim,
}

#[derive(Debug, Clone)]
pub struct InstField {
    pub name: u32,
    pub desc: u32,
    pub slot: u32,
}

#[derive(Debug, Clone)]
pub struct StaticField {
    pub name: u32,
    pub desc: u32,
    pub value: Value,
    pub is_enum_const: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Target {
    Dex { dex: u32, method_idx: u32 },
    Native { class: u32, name: u32, shorty: u32 },
}

#[derive(Clone)]
pub struct MethodEntry {
    pub class_id: u32,
    pub name: u32,
    pub shorty: u32,
    /// per-slot widths (1 or 2) in declaration order, excluding `this`
    pub params: Vec<u8>,
    /// 0 = void, 1 = single, 2 = wide
    pub ret: u8,
    pub is_static: bool,
    pub is_abstract: bool,
    pub target: Target,
    pub code: Option<Arc<MethodCode>>,
}

#[derive(Debug, Clone)]
pub struct RuntimeClass {
    pub id: u32,
    pub type_id: u32,
    pub name: String,
    /// resolved SUPERCLASS CLASS id (never a type id); None until finalized
    pub super_id: Option<u32>,
    /// unresolved super as a TYPE id (shims resolve lazily in finalize_shim)
    pub super_type: Option<u32>,
    pub interfaces: Vec<u32>,
    pub access: u32,
    pub kind: ClassKind,
    pub is_throwable: bool,
    pub instance_fields: Vec<InstField>,
    pub statics: Vec<StaticField>,
    pub own_methods: Vec<u32>,
    /// (name, shorty) -> global method idx (virtual dispatch, merged)
    pub vmap: HashMap<(u32, u32), u32>,
    /// (name, param slot count) -> global method idx (direct/static, merged)
    pub dmap: HashMap<(u32, u32), u32>,
    /// iface class id -> (name, shorty) -> method idx
    pub imap: HashMap<u32, HashMap<(u32, u32), u32>>,
    pub initialized: bool,
    pub init_in_progress: bool,
    pub finalized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTarget {
    Instance { class_id: u32, slot: u32 },
    Static { class_id: u32, index: u32 },
}

#[derive(Default)]
pub struct Registry {
    pub dexes: Vec<Arc<Dex>>,
    /// dex id whose methods are being decoded
    pub current_dex: u32,
    pub(crate) types: Vec<String>,
    pub(crate) type_map: HashMap<String, u32>,
    pub(crate) names: Vec<String>,
    pub(crate) name_map: HashMap<String, u32>,
    pub(crate) shorties: Vec<String>,
    pub(crate) shorty_map: HashMap<String, u32>,
    pub(crate) strings: Vec<String>,
    pub(crate) string_map: HashMap<String, u32>,
    pub classes: Vec<RuntimeClass>,
    pub(crate) class_by_type_map: HashMap<u32, u32>,
    pub methods: Vec<MethodEntry>,
    pub(crate) direct_cache: HashMap<(u32, u32), u32>,
    pub(crate) field_cache: HashMap<(u32, u32, u32), Option<FieldTarget>>,
    pub(crate) loading: Vec<u32>,
    pub natives: NativeRegistry,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    pub fn intern_type(&mut self, desc: &str) -> u32 {
        if let Some(&id) = self.type_map.get(desc) {
            return id;
        }
        let id = self.types.len() as u32;
        self.types.push(desc.to_string());
        self.type_map.insert(desc.to_string(), id);
        id
    }

    pub fn intern_name(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.name_map.get(name) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.to_string());
        self.name_map.insert(name.to_string(), id);
        id
    }

    pub fn intern_shorty(&mut self, shorty: &str) -> u32 {
        if let Some(&id) = self.shorty_map.get(shorty) {
            return id;
        }
        let id = self.shorties.len() as u32;
        self.shorties.push(shorty.to_string());
        self.shorty_map.insert(shorty.to_string(), id);
        id
    }

    pub fn intern_string(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.string_map.get(s) {
            return id;
        }
        let id = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.string_map.insert(s.to_string(), id);
        id
    }

    pub fn string_text(&self, id: u32) -> &str {
        self.strings.get(id as usize).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn type_str(&self, id: u32) -> &str {
        self.types.get(id as usize).map(|s| s.as_str()).unwrap_or("?")
    }

    pub fn name_str(&self, id: u32) -> &str {
        self.names.get(id as usize).map(|s| s.as_str()).unwrap_or("?")
    }

    pub fn shorty_str(&self, id: u32) -> &str {
        self.shorties.get(id as usize).map(|s| s.as_str()).unwrap_or("?")
    }

    pub fn name_id(&self, name: &str) -> Option<u32> {
        self.name_map.get(name).copied()
    }

    pub fn type_of_class(&self, class_id: u32) -> u32 {
        self.classes.get(class_id as usize).map(|c| c.type_id).unwrap_or(u32::MAX)
    }

    pub fn class_name(&self, class_id: u32) -> &str {
        self.classes.get(class_id as usize).map(|c| c.name.as_str()).unwrap_or("?")
    }

    pub fn class_by_type(&self, type_id: u32) -> Option<u32> {
        self.class_by_type_map.get(&type_id).copied()
    }

    pub fn class_by_name(&self, desc: &str) -> Option<u32> {
        let t = *self.type_map.get(desc)?;
        self.class_by_type(t)
    }

    pub fn add_dex(&mut self, dex: Arc<Dex>) -> u32 {
        let id = self.dexes.len() as u32;
        self.dexes.push(dex);
        id
    }

    /// Declare a framework shim class shell (fields/natives attach later).
    pub fn declare_shim(&mut self, desc: &str, super_desc: Option<&str>) -> u32 {
        let t = self.intern_type(desc);
        if let Some(&cid) = self.class_by_type_map.get(&t) {
            return cid;
        }
        let super_type = super_desc.map(|s| self.intern_type(s));
        let id = self.classes.len() as u32;
        self.classes.push(RuntimeClass {
            id,
            type_id: t,
            name: desc.to_string(),
            super_id: None,
            super_type,
            interfaces: Vec::new(),
            access: 0,
            kind: ClassKind::Shim,
            is_throwable: desc == "Ljava/lang/Throwable;",
            instance_fields: Vec::new(),
            statics: Vec::new(),
            own_methods: Vec::new(),
            vmap: HashMap::new(),
            dmap: HashMap::new(),
            imap: HashMap::new(),
            initialized: false,
            init_in_progress: false,
            finalized: false,
        });
        self.class_by_type_map.insert(t, id);
        id
    }

    pub fn add_shim_field(&mut self, class_id: u32, name: &str, desc: &str) {
        let nid = self.intern_name(name);
        let did = self.intern_type(desc);
        let cls = &mut self.classes[class_id as usize];
        let slot = cls.instance_fields.len() as u32;
        cls.instance_fields.push(InstField { name: nid, desc: did, slot });
    }

    pub fn add_shim_static(&mut self, class_id: u32, name: &str, desc: &str, value: Value) {
        let nid = self.intern_name(name);
        let did = self.intern_type(desc);
        self.classes[class_id as usize].statics.push(StaticField {
            name: nid,
            desc: did,
            value,
            is_enum_const: false,
        });
    }

    pub fn add_shim_interfaces(&mut self, class_id: u32, ifaces: &[&str]) {
        for d in ifaces {
            let t = self.intern_type(d);
            self.classes[class_id as usize].interfaces.push(t);
        }
    }

    pub fn set_shim_access(&mut self, class_id: u32, access: u32) {
        self.classes[class_id as usize].access = access;
    }

    /// Fully resolve a shim class: super chain, layout rebase, dispatch maps,
    /// native method materialization.
    fn finalize_shim(&mut self, class_id: u32) -> Result<(), VmFault> {
        if self.classes[class_id as usize].finalized {
            return Ok(());
        }
        let name = self.classes[class_id as usize].name.clone();
        let super_type = self.classes[class_id as usize].super_type;
        let super_class_id = match super_type {
            Some(st) => Some(self.ensure_class(st)?),
            None => None,
        };

        // rebase instance field slots after inherited ones
        if let Some(sc) = super_class_id {
            let inherited_len = self.classes[sc as usize].instance_fields.len();
            if inherited_len > 0 {
                let own = self.classes[class_id as usize].instance_fields.clone();
                let base = inherited_len as u32;
                let mut new_layout: Vec<InstField> =
                    self.classes[sc as usize].instance_fields.clone();
                for mut f in own {
                    f.slot += base;
                    new_layout.push(f);
                }
                self.classes[class_id as usize].instance_fields = new_layout;
            }
        }

        let mut dmap = HashMap::new();
        let mut vmap = HashMap::new();
        if let Some(sc) = super_class_id {
            dmap.extend(self.classes[sc as usize].dmap.clone());
            vmap.extend(self.classes[sc as usize].vmap.clone());
        }

        // materialize MethodEntries for natives registered on this class
        let registered = self.natives.for_class(&name);
        let mut own_methods = Vec::new();
        for (mname, mshorty, is_static, f) in registered.into_iter() {
            let nid = self.intern_name(&mname);
            let sid = self.intern_shorty(&mshorty);
            let params = shorty_slots(&mshorty);
            let slots = params.iter().sum::<u8>() as u32;
            let entry = MethodEntry {
                class_id,
                name: nid,
                shorty: sid,
                params,
                ret: shorty_ret(&mshorty),
                is_static,
                is_abstract: false,
                target: Target::Native { class: class_id, name: nid, shorty: sid },
                code: None,
            };
            let gidx = self.methods.len() as u32;
            self.methods.push(entry);
            own_methods.push(gidx);
            let _ = f;
            if is_static || mname == "<init>" {
                // statics and constructors dispatch through dmap
                dmap.insert((nid, slots), gidx);
            } else {
                vmap.insert((nid, sid), gidx);
            }
        }

        // interface dispatch maps
        let mut imap: HashMap<u32, HashMap<(u32, u32), u32>> = HashMap::new();
        let mut iface_class_ids = Vec::new();
        let declared_ifaces = self.classes[class_id as usize].interfaces.clone();
        for it in declared_ifaces {
            let ic = self.ensure_class(it)?;
            iface_class_ids.push(ic);
            if let Some(m) = self.classes[ic as usize].imap.get(&ic) {
                imap.entry(ic).or_insert_with(|| m.clone());
            }
            for (k, &v) in self.classes[ic as usize].vmap.iter() {
                vmap.entry(*k).or_insert(v);
            }
        }

        let throwable = self.is_throwable_chain(class_id);
        let cls = &mut self.classes[class_id as usize];
        cls.super_id = super_class_id;
        cls.dmap = dmap;
        cls.vmap = vmap;
        cls.imap = imap;
        cls.interfaces = iface_class_ids;
        cls.is_throwable = throwable;
        cls.own_methods = own_methods;
        cls.finalized = true;
        Ok(())
    }

    fn is_throwable_chain(&self, class_id: u32) -> bool {
        let mut cur = Some(class_id);
        while let Some(c) = cur {
            let cls = &self.classes[c as usize];
            if cls.name == "Ljava/lang/Throwable;" {
                return true;
            }
            cur = cls.super_id;
        }
        false
    }

    /// Load (or fetch already-loaded) class by interned type id.
    pub fn ensure_class(&mut self, type_id: u32) -> Result<u32, VmFault> {
        if let Some(&cid) = self.class_by_type_map.get(&type_id) {
            if matches!(self.classes[cid as usize].kind, ClassKind::Shim)
                && !self.classes[cid as usize].finalized
            {
                self.finalize_shim(cid)?;
            }
            return Ok(cid);
        }
        let desc = self.type_str(type_id).to_string();
        if self.loading.contains(&type_id) {
            return Err(VmFault::Vm(format!("class cycle loading {desc}")));
        }
        let mut found: Option<(u32, u32)> = None;
        for (di, dex) in self.dexes.iter().enumerate() {
            if let Some(def) = dex.find_class(&desc) {
                found = Some((di as u32, def));
                break;
            }
        }
        self.loading.push(type_id);
        let result = match found {
            Some((dex_id, def)) => self.load_dex_class(type_id, dex_id, def),
            None => Err(VmFault::Vm(format!(
                "class not found: {desc} (no dex definition, no shim declared)"
            ))),
        };
        self.loading.pop();
        result
    }

    fn load_dex_class(&mut self, type_id: u32, dex_id: u32, def: u32) -> Result<u32, VmFault> {
        if let Some(&cid) = self.class_by_type_map.get(&type_id) {
            return Ok(cid);
        }
        let dex = self.dexes[dex_id as usize].clone();
        let cd = dex
            .class_defs
            .get(def as usize)
            .ok_or_else(|| VmFault::Vm(format!("bad class def {def}")))?;
        let desc = dex.type_name(cd.class_idx).unwrap_or("?").to_string();

        let super_type: Option<u32> = if cd.superclass_idx != litebox_dex::NO_INDEX {
            let sdesc = dex.type_name(cd.superclass_idx).unwrap_or("?").to_string();
            Some(self.intern_type(&sdesc))
        } else {
            None
        };
        let iface_type_ids: Vec<u32> = cd
            .interfaces
            .iter()
            .filter_map(|&it| dex.type_name(it).ok().map(|s| self.intern_type(s)))
            .collect();

        let id = self.classes.len() as u32;
        self.classes.push(RuntimeClass {
            id,
            type_id,
            name: desc.clone(),
            super_id: None,
            super_type,
            interfaces: iface_type_ids.clone(),
            access: cd.access_flags,
            kind: ClassKind::Dex { dex: dex_id, def },
            is_throwable: desc == "Ljava/lang/Throwable;",
            instance_fields: Vec::new(),
            statics: Vec::new(),
            own_methods: Vec::new(),
            vmap: HashMap::new(),
            dmap: HashMap::new(),
            imap: HashMap::new(),
            initialized: false,
            init_in_progress: false,
            finalized: false,
        });
        self.class_by_type_map.insert(type_id, id);

        let super_class_id = match super_type {
            Some(st) => Some(self.ensure_class(st)?),
            None => None,
        };
        let mut iface_class_ids = Vec::with_capacity(iface_type_ids.len());
        for &it in &iface_type_ids {
            iface_class_ids.push(self.ensure_class(it)?);
        }

        // layout: inherited fields first, then own
        let mut layout: Vec<InstField> = Vec::new();
        if let Some(sc) = super_class_id {
            layout.extend(self.classes[sc as usize].instance_fields.clone());
        }
        for f in &cd.instance_fields {
            let fd = &dex.field_ids[f.field_idx as usize];
            let nid = self.intern_name(dex.string(fd.name_idx).unwrap_or("?"));
            let did = self.intern_type(dex.type_name(fd.type_idx as u32).unwrap_or("?"));
            let slot = layout.len() as u32;
            layout.push(InstField { name: nid, desc: did, slot });
        }

        let mut statics: Vec<StaticField> = Vec::new();
        for f in &cd.static_fields {
            let fd = &dex.field_ids[f.field_idx as usize];
            let nid = self.intern_name(dex.string(fd.name_idx).unwrap_or("?"));
            let tdesc = dex.type_name(fd.type_idx as u32).unwrap_or("?").to_string();
            let did = self.intern_type(&tdesc);
            let pk = PrimKind::from_descriptor(&tdesc);
            let value = if pk.is_wide() { Value::L(0) } else { zero_for(pk) };
            statics.push(StaticField { name: nid, desc: did, value, is_enum_const: false });
        }

        let mut dmap: HashMap<(u32, u32), u32> = HashMap::new();
        let mut vmap: HashMap<(u32, u32), u32> = HashMap::new();
        if let Some(sc) = super_class_id {
            dmap.extend(self.classes[sc as usize].dmap.clone());
            vmap.extend(self.classes[sc as usize].vmap.clone());
        }
        let mut own_methods = Vec::new();
        self.current_dex = dex_id;
        for (virtual_group, list) in [(false, &cd.direct_methods), (true, &cd.virtual_methods)] {
            for em in list {
                let midx = em.method_idx;
                let m = &dex.method_ids[midx as usize];
                let nid = self.intern_name(dex.string(m.name_idx).unwrap_or("?"));
                let proto = &dex.protos[m.proto_idx as usize];
                let shorty = dex.string(proto.shorty_idx).unwrap_or("?").to_string();
                let sid = self.intern_shorty(&shorty);
                let mut params = Vec::new();
                for &pt in &proto.params {
                    let pd = dex.type_name(pt).unwrap_or("?").to_string();
                    let wide = pd == "J" || pd == "D";
                    params.push(if wide { 2 } else { 1 });
                }
                let ret = {
                    let rd = dex.type_name(proto.return_type_idx).unwrap_or("?").to_string();
                    if rd == "J" || rd == "D" {
                        2
                    } else if rd == "V" {
                        0
                    } else {
                        1
                    }
                };
                let is_static = em.access_flags & ACC_STATIC != 0;
                let is_native = em.access_flags & ACC_NATIVE != 0;
                let is_abstract = em.access_flags & ACC_ABSTRACT != 0;
                let (target, code) = if is_native {
                    (Target::Native { class: id, name: nid, shorty: sid }, None)
                } else if is_abstract {
                    (Target::Dex { dex: dex_id, method_idx: midx }, None)
                } else {
                    let ci = em.code.as_ref().ok_or_else(|| {
                        VmFault::Vm(format!(
                            "method {desc}.{} has no code, is not abstract or native",
                            self.name_str(nid)
                        ))
                    })?;
                    let mc = decode_method(&dex, self, ci, &shorty)
                        .map_err(|e| VmFault::Vm(format!("decode {desc}.{:?}: {}", self.name_str(nid), e.0)))?;
                    (Target::Dex { dex: dex_id, method_idx: midx }, Some(Arc::new(mc)))
                };
                let slots = params.iter().sum::<u8>() as u32;
                let entry = MethodEntry {
                    class_id: id,
                    name: nid,
                    shorty: sid,
                    params,
                    ret,
                    is_static,
                    is_abstract,
                    target,
                    code,
                };
                let gidx = self.methods.len() as u32;
                self.methods.push(entry);
                own_methods.push(gidx);
                if virtual_group {
                    vmap.insert((nid, sid), gidx);
                } else {
                    dmap.insert((nid, slots), gidx);
                }
            }
        }

        // interface default methods: collect transitively
        let mut all_ifaces: Vec<u32> = iface_class_ids.clone();
        let mut i = 0;
        while i < all_ifaces.len() {
            let ic = all_ifaces[i];
            let supers: Vec<u32> = self.classes[ic as usize].interfaces.clone();
            all_ifaces.extend(supers);
            i += 1;
        }
        let mut imap: HashMap<u32, HashMap<(u32, u32), u32>> = HashMap::new();
        for &ic in &all_ifaces {
            let mut m: HashMap<(u32, u32), u32> = HashMap::new();
            for &gm in &self.classes[ic as usize].own_methods {
                let e = &self.methods[gm as usize];
                if !e.is_abstract && self.classes[e.class_id as usize].access & ACC_INTERFACE != 0 {
                    m.insert((e.name, e.shorty), gm);
                }
            }
            for (k, &v) in self.classes[ic as usize].vmap.iter() {
                m.entry(*k).or_insert(v);
            }
            if !m.is_empty() {
                imap.insert(ic, m);
            }
        }
        let defaults: Vec<((u32, u32), u32)> =
            imap.values().flat_map(|m| m.iter().map(|(k, &v)| (*k, v))).collect();
        for (k, v) in defaults {
            vmap.entry(k).or_insert(v);
        }
        if let Some(sc) = super_class_id {
            for (k, v) in self.classes[sc as usize].imap.clone() {
                imap.entry(k).or_insert(v);
            }
        }

        let throwable = self.is_throwable_chain(id);
        let cls = &mut self.classes[id as usize];
        cls.instance_fields = layout;
        cls.statics = statics;
        cls.own_methods = own_methods;
        cls.dmap = dmap;
        cls.vmap = vmap;
        cls.imap = imap;
        cls.interfaces = iface_class_ids;
        cls.super_id = super_class_id;
        cls.is_throwable = throwable;
        cls.finalized = true;
        Ok(id)
    }

    /// Resolve invoke-direct/static: (dex, method_idx) -> global method idx.
    pub fn resolve_direct(&mut self, dex: u32, method_idx: u32) -> Result<u32, VmFault> {
        if let Some(&g) = self.direct_cache.get(&(dex, method_idx)) {
            return Ok(g);
        }
        let mids = self.dexes[dex as usize].method_ids[method_idx as usize].clone();
        let cls_desc = self.dexes[dex as usize]
            .type_name(mids.class_idx as u32)
            .unwrap_or("?")
            .to_string();
        let tid = self.intern_type(&cls_desc);
        let cid = self.ensure_class(tid)?;
        let name = self.dexes[dex as usize].string(mids.name_idx).unwrap_or("?").to_string();
        let nid = self.intern_name(&name);
        let proto = &self.dexes[dex as usize].protos[mids.proto_idx as usize];
        let mut slots = 0u32;
        for &pt in &proto.params {
            let pd = self.dexes[dex as usize].type_name(pt).unwrap_or("?").to_string();
            slots += if pd == "J" || pd == "D" { 2 } else { 1 };
        }
        let mut cur = Some(cid);
        let mut found = None;
        while let Some(c) = cur {
            if let Some(&g) = self.classes[c as usize].dmap.get(&(nid, slots)) {
                found = Some(g);
                break;
            }
            cur = self.classes[c as usize].super_id;
        }
        let g = found.ok_or_else(|| {
            VmFault::Vm(format!(
                "direct method not found: {}.{} ({slots} arg slots)",
                self.class_name(cid),
                name
            ))
        })?;
        self.direct_cache.insert((dex, method_idx), g);
        Ok(g)
    }

    /// Resolve a field reference to a concrete target, cached.
    pub fn resolve_field(
        &mut self,
        type_id: u32,
        name: u32,
        desc: u32,
    ) -> Result<FieldTarget, VmFault> {
        let key = (type_id, name, desc);
        if let Some(&t) = self.field_cache.get(&key) {
            return t.ok_or_else(|| {
                VmFault::Vm(format!(
                    "field not found: {}.{}:{}",
                    self.type_str(type_id),
                    self.name_str(name),
                    self.type_str(desc)
                ))
            });
        }
        let cid = self.ensure_class(type_id)?;
        let mut cur = Some(cid);
        let mut inst = None;
        let mut stat = None;
        while let Some(c) = cur {
            let cls = &self.classes[c as usize];
            if inst.is_none() {
                for f in &cls.instance_fields {
                    if f.name == name && f.desc == desc {
                        inst = Some(FieldTarget::Instance { class_id: c, slot: f.slot });
                        break;
                    }
                }
            }
            if stat.is_none() {
                for (i, s) in cls.statics.iter().enumerate() {
                    if s.name == name && s.desc == desc {
                        stat = Some(FieldTarget::Static { class_id: c, index: i as u32 });
                        break;
                    }
                }
            }
            if inst.is_some() && stat.is_some() {
                break;
            }
            cur = cls.super_id;
        }
        let t = inst.or(stat);
        self.field_cache.insert(key, t);
        t.ok_or_else(|| {
            VmFault::Vm(format!(
                "field not found: {}.{}:{}",
                self.class_name(cid),
                self.name_str(name),
                self.type_str(desc)
            ))
        })
    }

    pub fn subtype(&self, class_id: u32, of_type: u32) -> bool {
        let mut stack = vec![class_id];
        while let Some(c) = stack.pop() {
            if c == of_type {
                return true;
            }
            let cls = &self.classes[c as usize];
            for &ifc in &cls.interfaces {
                if self.iface_extends(ifc, of_type) {
                    return true;
                }
            }
            if let Some(s) = cls.super_id {
                stack.push(s);
            }
        }
        false
    }

    fn iface_extends(&self, iface: u32, of_type: u32) -> bool {
        if iface == of_type {
            return true;
        }
        let cls = &self.classes[iface as usize];
        cls.interfaces.iter().any(|&i| self.iface_extends(i, of_type))
    }

    pub fn is_abstract_class(&self, class_id: u32) -> bool {
        self.classes[class_id as usize].access & ACC_ABSTRACT != 0
    }

    pub fn is_interface(&self, class_id: u32) -> bool {
        self.classes[class_id as usize].access & ACC_INTERFACE != 0
    }

    pub fn instance_slots(&self, class_id: u32) -> usize {
        self.classes[class_id as usize].instance_fields.len()
    }

    pub fn get_static(&self, ft: FieldTarget) -> Value {
        match ft {
            FieldTarget::Static { class_id, index } => {
                self.classes[class_id as usize].statics[index as usize].value
            }
            _ => crate::value::NULL,
        }
    }

    pub fn set_static(&mut self, ft: FieldTarget, v: Value) {
        if let FieldTarget::Static { class_id, index } = ft {
            self.classes[class_id as usize].statics[index as usize].value = v;
        }
    }

    pub fn method_at(&self, i: u32) -> Option<&MethodEntry> {
        self.methods.get(i as usize)
    }

    pub fn methods_len(&self) -> usize {
        self.methods.len()
    }

    /// Rebuild lookup maps after bulk state restoration.
    pub fn rebuild_maps(&mut self) {
        self.type_map = self
            .types
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), i as u32))
            .collect();
        self.name_map =
            self.names.iter().enumerate().map(|(i, t)| (t.clone(), i as u32)).collect();
        self.shorty_map =
            self.shorties.iter().enumerate().map(|(i, t)| (t.clone(), i as u32)).collect();
        self.string_map =
            self.strings.iter().enumerate().map(|(i, t)| (t.clone(), i as u32)).collect();
        self.class_by_type_map =
            self.classes.iter().map(|c| (c.type_id, c.id)).collect();
        self.direct_cache.clear();
        self.field_cache.clear();
    }

    pub fn all_class_names(&self) -> Vec<String> {
        self.classes.iter().map(|c| c.name.clone()).collect()
    }
}

pub fn zero_for(kind: PrimKind) -> Value {
    match kind {
        PrimKind::J => Value::L(0),
        PrimKind::F => Value::F(0.0),
        PrimKind::D => Value::D(0.0),
        PrimKind::Ref => crate::value::NULL,
        _ => Value::I(0),
    }
}

/// slot widths from a shorty string (skip return type at index 0)
pub fn shorty_slots(shorty: &str) -> Vec<u8> {
    shorty
        .chars()
        .skip(1)
        .map(|c| if c == 'J' || c == 'D' { 2 } else { 1 })
        .collect()
}

pub fn shorty_ret(shorty: &str) -> u8 {
    match shorty.chars().next() {
        Some('V') => 0,
        Some('J') | Some('D') => 2,
        _ => 1,
    }
}

pub fn throw_spec(desc: &str, msg: impl Into<String>) -> ThrowSpec {
    ThrowSpec::new(desc, msg)
}
