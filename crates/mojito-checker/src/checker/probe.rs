//! Checking speculatively: an overload candidate is checked inside a probe,
//! which journals every fact-store write and undoes it when the probe
//! closes, so only the selected candidate's re-run leaves facts — as
//! upstream's checked IR holds the chosen call's ops alone.

use super::Checker;
use super::body_carry::carried_stores;
use mojito_checked::fact_store::ProbeMark;

impl Checker {
    /// Run `check`, then undo every fact it recorded, whether it succeeded
    /// or not. Probes nest.
    pub(super) fn probe<R>(&self, check: impl FnOnce() -> R) -> R {
        let marks = self.open_probe_marks();
        let result = check();
        self.roll_back_to(marks);
        result
    }
}

macro_rules! define_probe {
    ($($loc:ident $kind:ident $carry:ident = $field:ident;)*) => {
        /// Every store's position when a probe opened: the carried stores,
        /// the deletability verdicts beside them, and the per-call bindings.
        struct ProbeMarks {
            $($carry: ProbeMark,)*
            linear_declarations: ProbeMark,
            linear_bindings: ProbeMark,
            call_bindings: ProbeMark,
        }

        impl Checker {
            fn open_probe_marks(&self) -> ProbeMarks {
                let mut deletability = self.explicit_destroy_deletability.borrow_mut();
                ProbeMarks {
                    $($carry: self.$field.borrow_mut().open_probe(),)*
                    linear_declarations: deletability.declarations.open_probe(),
                    linear_bindings: deletability.bindings.open_probe(),
                    call_bindings: self.call_bindings.borrow_mut().open_probe(),
                }
            }

            fn roll_back_to(&self, marks: ProbeMarks) {
                $(self.$field.borrow_mut().roll_back(marks.$carry);)*
                let mut deletability = self.explicit_destroy_deletability.borrow_mut();
                deletability.declarations.roll_back(marks.linear_declarations);
                deletability.bindings.roll_back(marks.linear_bindings);
                self.call_bindings.borrow_mut().roll_back(marks.call_bindings);
            }
        }
    };
}
carried_stores!(define_probe);
