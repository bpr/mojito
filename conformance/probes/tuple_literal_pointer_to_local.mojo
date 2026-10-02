# PROBE: a tuple literal holding a pointer to a local.
#
# **Differs.** The pin prints 2. Mojito rejects it with "not a compile-time
# value: type pack contains a type which cannot be materialized in source".
# Filed in `docs/roadmap.md` §3 (3.95). When it runs, promote it to
# `assets/ok`.
#
# Observed 2026-09-29 against the pinned Mojo.
#
# Run:    mojo run tuple_literal_pointer_to_local.mojo
def main():
    var x = 1
    var p = (Pointer(to=x), 2)
    print(p[1])
