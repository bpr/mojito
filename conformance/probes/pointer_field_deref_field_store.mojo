# Question: a field store through the dereferenced pointer field of a view.
# Upstream (`1.1.0.dev2026082605`) runs it: `9`.
#
# Mojito fails at the VM: "cannot index ref". The store place lowers as
# `p.src[0].v` (root, pointer field, deref index, field), and the VM's place
# writer cannot project a field through a pointer value it reaches by index;
# a whole store through the same deref (`p.src[] = Box(11)`,
# `assets/ok/call_result_pointer_deref_store.mojo`) and a field store through
# a plain pointer local (`q[].v = 9`) both run.
#
# On the fix: promote this file to
# `assets/ok/pointer_field_deref_field_store.mojo` with its manifest rows,
# and delete the `pointer-field-deref-field-store` ledger row in
# docs/roadmap.md.
@fieldwise_init
struct Box:
    var v: Int

@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Box, Self.o]

def main():
    var b = Box(7)
    var p = P(Pointer(to=b))
    p.src[].v = 9
    print(b.v)
