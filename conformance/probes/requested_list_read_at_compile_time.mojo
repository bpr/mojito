# PROBE: a body's requested `List` binding read by a compile-time expression.
#
# The pin prints 3 and 3; Mojito stops with "vm backend does not support
# methods on None yet": `comptime L = mk()` is a request, and `len(L)` reads
# it where no value has been demanded yet.
#
# Observed 2026-10-08 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run requested_list_read_at_compile_time.mojo
#         cargo run -- run conformance/probes/requested_list_read_at_compile_time.mojo
def mk() -> List[Int]:
    return [1, 2, 3]

def main():
    comptime L = mk()
    print(len(materialize[L]()))
    print(comptime(len(L)))
