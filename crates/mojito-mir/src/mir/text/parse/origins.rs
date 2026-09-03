//! Origin and reference-signature decoding.

use super::*;

impl Decoder {
    pub(super) fn origin_path(&mut self, value: &Value) -> Vec<OriginSeg> {
        self.list(value)
            .map(|values| values.iter().filter_map(|v| self.origin_seg(v)).collect())
            .unwrap_or_default()
    }

    pub(super) fn origin_seg(&mut self, value: &Value) -> Option<OriginSeg> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "any_index" => Some(OriginSeg::AnyIndex),
            ValueKind::Atom(tag) if tag == "subtree" => Some(OriginSeg::Subtree),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "field" => self.symbol(inner).map(OriginSeg::Field),
                "interior" => self.symbol(inner).map(OriginSeg::Interior),
                other => {
                    self.error(value.span, format!("unknown origin segment `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected origin segment");
                None
            }
        }
    }

    pub(super) fn origin(&mut self, value: &Value) -> Option<Origin> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "origin_self" => Some(Origin::SelfParam),
            ValueKind::Atom(tag) if tag == "origin_static" => Some(Origin::Static),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "origin_param" => self.uint32(inner).map(|v| Origin::Param(OriginParamId(v))),
                "origin_union" => {
                    let members = self
                        .list(inner)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|v| self.origin(v))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Some(Origin::union(members))
                }
                other => {
                    self.error(value.span, format!("unknown origin `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, _) if tag == "origin_place" => {
                self.origin_place(value).map(Origin::Place)
            }
            ValueKind::Record(tag, fields) if tag == "origin_untracked" => {
                let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                self.unknown(fields, &["mutable"]);
                Some(Origin::Untracked { mutable })
            }
            _ => {
                self.error(value.span, "expected origin");
                None
            }
        }
    }

    pub(super) fn origin_place(&mut self, value: &Value) -> Option<OriginPlace> {
        let fields = self.record(value, "origin_place").ok()?;
        let root = self.req(value, fields, "root", |d, v| d.identity(Some(v), "$v"))?;
        let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
        self.unknown(fields, &["root", "path"]);
        Some(OriginPlace {
            root: OwnerId(root as u32),
            path,
        })
    }

    pub(super) fn pointer_origin(&mut self, value: &Value) -> Option<PointerOrigin> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "pointer_static" => Some(PointerOrigin::Static),
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "pointer_place" => {
                    let place = self.req(value, fields, "place", Self::origin_place)?;
                    let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                    self.unknown(fields, &["place", "mutable"]);
                    Some(PointerOrigin::Place { place, mutable })
                }
                "pointer_param" => {
                    let id = OriginParamId(self.req(value, fields, "id", Self::uint32)?);
                    let mutability = self.req(value, fields, "mutability", Self::mutability)?;
                    let interior =
                        self.req(value, fields, "interior", |d, v| Some(d.strings(v)))?;
                    let subtree = self.req(value, fields, "subtree", Self::boolean)?;
                    self.unknown(fields, &["id", "mutability", "interior", "subtree"]);
                    Some(PointerOrigin::Param {
                        id,
                        mutability,
                        interior,
                        subtree,
                    })
                }
                "pointer_self" => {
                    let mutability = self.req(value, fields, "mutability", Self::mutability)?;
                    let interior =
                        self.req(value, fields, "interior", |d, v| Some(d.strings(v)))?;
                    let subtree = self.req(value, fields, "subtree", Self::boolean)?;
                    self.unknown(fields, &["mutability", "interior", "subtree"]);
                    Some(PointerOrigin::SelfPlace {
                        mutability,
                        interior,
                        subtree,
                    })
                }
                "pointer_untracked" | "pointer_unsafe_any" => {
                    let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                    self.unknown(fields, &["mutable"]);
                    Some(if tag == "pointer_untracked" {
                        PointerOrigin::Untracked { mutable }
                    } else {
                        PointerOrigin::UnsafeAny { mutable }
                    })
                }
                other => {
                    self.error(value.span, format!("unknown pointer origin `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected pointer origin");
                None
            }
        }
    }

    pub(super) fn ref_ty(&mut self, value: &Value) -> Option<RefTy> {
        let fields = self.record(value, "ref").ok()?;
        let referent = Box::new(self.req(value, fields, "referent", Self::ty)?);
        let origin = self.req(value, fields, "origin", Self::origin)?;
        let mutability = self.req(value, fields, "mutability", Self::mutability)?;
        self.unknown(fields, &["referent", "origin", "mutability"]);
        Some(RefTy {
            referent,
            origin,
            mutability,
        })
    }

    pub(super) fn mutability(&mut self, value: &Value) -> Option<Mutability> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "immutable" => Some(Mutability::Immutable),
            ValueKind::Atom(tag) if tag == "mutable" => Some(Mutability::Mutable),
            ValueKind::Positional(tag, inner) if tag == "mutability_param" => self
                .uint32(inner)
                .map(|v| Mutability::Param(OriginParamId(v))),
            _ => {
                self.error(value.span, "expected mutability");
                None
            }
        }
    }

    pub(super) fn ref_sigs(&mut self, value: &Value) -> Vec<Option<RefSig>> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .map(|v| self.option_value(Some(v)).and_then(|v| self.ref_sig(v)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn ref_sig(&mut self, value: &Value) -> Option<RefSig> {
        let fields = self.record(value, "ref_sig").ok()?;
        let origin = self.req(value, fields, "origin", Self::sig_origin)?;
        let mutability = self.req(value, fields, "mutability", Self::sig_mutability)?;
        self.unknown(fields, &["origin", "mutability"]);
        Some(RefSig { origin, mutability })
    }

    pub(super) fn sig_origin(&mut self, value: &Value) -> Option<SigOrigin> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "sig_self" => Some(SigOrigin::Self_),
            ValueKind::Atom(tag) if tag == "sig_static" => Some(SigOrigin::Static),
            ValueKind::Atom(tag) if tag == "sig_infer" => Some(SigOrigin::Infer),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "sig_param" => self.uint(inner).map(SigOrigin::Param),
                "sig_bound" => self.origin(inner).map(SigOrigin::Bound),
                "sig_union" => {
                    let members = self
                        .list(inner)
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|v| self.sig_origin(v))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Some(SigOrigin::Union(members))
                }
                other => {
                    self.error(value.span, format!("unknown signature origin `{other}`"));
                    None
                }
            },
            ValueKind::Record(tag, fields) => match tag.as_str() {
                "sig_untracked" => {
                    let mutable = self.req(value, fields, "mutable", Self::boolean)?;
                    self.unknown(fields, &["mutable"]);
                    Some(SigOrigin::Untracked { mutable })
                }
                "sig_projected" => {
                    let base = Box::new(self.req(value, fields, "base", Self::sig_origin)?);
                    let path = self.req(value, fields, "path", |d, v| Some(d.origin_path(v)))?;
                    self.unknown(fields, &["base", "path"]);
                    Some(SigOrigin::Projected(base, path))
                }
                other => {
                    self.error(value.span, format!("unknown signature origin `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected signature origin");
                None
            }
        }
    }

    pub(super) fn sig_mutability(&mut self, value: &Value) -> Option<SigMutability> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "sig_immutable" => Some(SigMutability::Immutable),
            ValueKind::Atom(tag) if tag == "sig_mutable" => Some(SigMutability::Mutable),
            ValueKind::Atom(tag) if tag == "sig_infer" => Some(SigMutability::Infer),
            ValueKind::Positional(tag, inner) if tag == "sig_bool_param" => {
                self.uint(inner).map(SigMutability::BoolParam)
            }
            _ => {
                self.error(value.span, "expected signature mutability");
                None
            }
        }
    }

    pub(super) fn environment(&mut self, value: &Value) -> Option<CallableEnvironment> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "default" => Some(CallableEnvironment::Default),
            ValueKind::Atom(tag) if tag == "thin" => Some(CallableEnvironment::Thin),
            ValueKind::Positional(tag, inner) if tag == "capturing" => {
                self.capture_set(inner).map(CallableEnvironment::Capturing)
            }
            _ => {
                self.error(value.span, "expected callable environment");
                None
            }
        }
    }

    pub(super) fn capture_set(&mut self, value: &Value) -> Option<CaptureOriginSet> {
        match &value.kind {
            ValueKind::Atom(tag) if tag == "capture_set_infer" => Some(CaptureOriginSet::Infer),
            ValueKind::Positional(tag, inner) => match tag.as_str() {
                "capture_set_param" => self
                    .uint32(inner)
                    .map(|v| CaptureOriginSet::Param(CaptureSetParamId(v))),
                "capture_set" => {
                    let mut captures = Vec::new();
                    if let Ok(values) = self.list(inner) {
                        for entry in values {
                            let Ok(fields) = self.record(entry, "capture_origin") else {
                                continue;
                            };
                            let origin = self
                                .required(entry, fields, "origin")
                                .and_then(|v| self.origin(v));
                            let access = self
                                .required(entry, fields, "access")
                                .and_then(|v| self.capture_access_kind(v));
                            self.unknown(fields, &["origin", "access"]);
                            if let (Some(origin), Some(access)) = (origin, access) {
                                captures.push(CaptureOrigin { origin, access });
                            }
                        }
                    }
                    Some(CaptureOriginSet::concrete(captures))
                }
                other => {
                    self.error(value.span, format!("unknown capture set `{other}`"));
                    None
                }
            },
            _ => {
                self.error(value.span, "expected capture set");
                None
            }
        }
    }

    pub(super) fn capture_access_kind(&mut self, value: &Value) -> Option<CaptureAccess> {
        match self.atom(value)? {
            "read" => Some(CaptureAccess::Read),
            "write" => Some(CaptureAccess::Write),
            other => {
                self.error(value.span, format!("unknown capture access `{other}`"));
                None
            }
        }
    }

    pub(super) fn convention(&mut self, value: &Value) -> Option<ArgConvention> {
        match self.atom(value)? {
            "imm" => Some(ArgConvention::Imm),
            "var" => Some(ArgConvention::Var),
            "mut" => Some(ArgConvention::Mut),
            "out" => Some(ArgConvention::Out),
            "ref" => Some(ArgConvention::Ref),
            "deinit" => Some(ArgConvention::Deinit),
            other => {
                self.error(value.span, format!("unknown convention `{other}`"));
                None
            }
        }
    }

    pub(super) fn conventions(&mut self, value: &Value) -> Vec<Option<ArgConvention>> {
        self.list(value)
            .map(|values| {
                values
                    .iter()
                    .map(|v| self.option_value(Some(v)).and_then(|v| self.convention(v)))
                    .collect()
            })
            .unwrap_or_default()
    }
}
