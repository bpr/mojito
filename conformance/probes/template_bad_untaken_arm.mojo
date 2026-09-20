# PROBE (re-probe): an invalid untaken arm in a declaration nothing instantiates.
#
# Both compilers reject it from the template, before any instance exists: the
# pin with "cannot implicitly convert 'StringLiteral[\"twenty-two\"]' value to
# 'Int'", Mojito with "type mismatch for return: expected Int, found
# StringLiteral". The enforced claim is
# `assets/type_error/untaken_comptime_if_type_error.mojo`; this file adds the
# never-instantiated shape, and pins that a checked template's facts are never
# a reason to skip the all-arm check.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run template_bad_untaken_arm.mojo
#         cargo run -- run conformance/probes/template_bad_untaken_arm.mojo
def choose[flag: Bool]() -> Int:
    comptime if flag:
        return 11
    else:
        return "twenty-two"


def main():
    print(1)
