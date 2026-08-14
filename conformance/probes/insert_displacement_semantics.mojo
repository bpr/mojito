# PROBE (semantics pinning): displacement-returning `insert` at the audited
# head — the return type (`Optional[V]`?), whether replacing an existing
# key keeps the ORIGINAL key or stores the new equal key, whether
# `Set.insert` replaces the stored element, and insertion-position
# retention.
#
# Context: nightly §6. Mojito: Dict/StringDict `insert(key, value)`
# replaces the whole entry (new key stored) and returns the displaced
# value; `Set.insert` replaces the stored element and returns the previous
# one; insertion position is retained.
#
# Run:    mojo run insert_displacement_semantics.mojo
#         cargo run -- run conformance/probes/insert_displacement_semantics.mojo
#
# If the head keeps the original key or does not replace Set elements:
#   adjust dict.mojo/set.mojo/string_dict.mojo `insert` bodies and the
#   family fixture/vm_test expectations.
def main():
    var d: Dict[Int, Int] = {1: 10}
    print(d.insert(1, 11).or_else(-1))
    var s: Set[Int] = {7}
    print(s.insert(7).or_else(-1))
