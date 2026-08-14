# PROBE (vocabulary pinning): the audited head's name for the mut-receiver
# take on UnsafeMaybeUninit (move the payload out, storage stays reusable).
# Mojito added `unsafe_take(mut self) -> T where Movable` beside the
# receiver-overloaded `unsafe_assume_init` pair; the upstream spelling is
# unverified.
#
# Run:    mojo run unsafe_maybe_uninit_take.mojo
#         cargo run -- run conformance/probes/unsafe_maybe_uninit_take.mojo
#
# If the head names it differently (or omits it): rename/remove the wrapper
#   in stdlib/std/memory.mojo and the vm_test
#   unsafe_maybe_uninit_init_with_places_and_takes expectations, and align
#   the `unsafe_init_with` spelling at the same time.
from memory import UnsafeMaybeUninit

def main():
    var u = UnsafeMaybeUninit[Int](5)
    print(u.unsafe_take())
    u.unsafe_write(6)
    print(u.unsafe_take())
