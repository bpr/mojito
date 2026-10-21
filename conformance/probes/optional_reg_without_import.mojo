# PROBE: `OptionalReg` spelled without an import (roadmap R817).
#
# The pin rejects it: "use of unknown declaration 'OptionalReg'; did you mean
# to import it from 'std.collections'? Add 'from std.collections import
# OptionalReg'". Mojito's prelude exports `OptionalReg`, so it accepts the
# program and prints 4.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run optional_reg_without_import.mojo
#         cargo run -- run conformance/probes/optional_reg_without_import.mojo
def main():
    var r = OptionalReg[Int](4)
    print(r.value())
