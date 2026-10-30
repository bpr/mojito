//! Lane-width scans: whether a struct method constructs a vector at a lane
//! its own parameters spell.

use crate::ast::{Expr, ExprKind, Method, Type, TypeParam};

/// Whether a struct method's template body constructs a vector at a lane its
/// own parameters spell (`Scalar[dt](x)`, `SIMD[DType.int32, w](v)`).
///
/// Such a construction checks with the lane symbolic: the struct's template
/// serves every call, the elaborator closing the lane per call.
pub fn method_constructs_at_own_lane(method: &Method) -> bool {
    struct Finder<'a> {
        own: &'a [TypeParam],
        found: bool,
    }

    struct Names<'a> {
        own: &'a [TypeParam],
        found: bool,
    }

    impl crate::visit::Visitor for Names<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            if let ExprKind::Identifier(name) = &expr.kind {
                self.found |= self.own.iter().any(|parameter| &parameter.name == name);
            }
        }

        fn visit_type(&mut self, ty: &Type) {
            if let Type::Named(name, arguments) = ty
                && arguments.is_empty()
            {
                self.found |= self.own.iter().any(|parameter| &parameter.name == name);
            }
        }
    }

    impl crate::visit::Visitor for Finder<'_> {
        fn visit_expr(&mut self, expr: &Expr) {
            let ExprKind::Call {
                name, param_args, ..
            } = &expr.kind
            else {
                return;
            };
            if name != "SIMD" && name != "Scalar" {
                return;
            }
            let mut names = Names {
                own: self.own,
                found: false,
            };
            for argument in param_args {
                crate::visit::walk_param_arg(&mut names, argument);
            }
            self.found |= names.found;
        }
    }

    let mut finder = Finder {
        own: &method.type_params,
        found: false,
    };
    crate::visit::walk_block(&mut finder, &method.body);
    finder.found
}
