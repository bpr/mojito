# PROBE: a mutable pointer written into storage whose element type embeds
# the same origin.
#
# Mojito prints 7 twice; the pin rejects the write with "aliasing values
# passed mutably to 'self' argument and passed mutably to 'value' argument in
# 'unsafe_write' call". Mojito accepts a program the pin rejects, so this is a
# divergence.
#
# Observed 2026-09-27 against `Mojo 1.6.0.dev2026092105`.
#
# Run:    mojo run pointer_write_aliasing_embedded_origin.mojo
#         cargo run -- run conformance/probes/pointer_write_aliasing_embedded_origin.mojo
from std.memory.alloc import unsafe_alloc


def main():
    var x = 7
    var p = unsafe_alloc[Pointer[Int, origin_of(x)]](1)
    p.unsafe_write(Pointer(to=x))
    print(p.unsafe_offset(0)[][])
    p.unsafe_free()
    print(x)
