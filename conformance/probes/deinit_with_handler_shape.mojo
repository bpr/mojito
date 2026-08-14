# PROBE (API-shape pinning): the exact `deinit_with`/`clear_with` handler
# signatures and drain order at the audited head — runtime funarg
# (`xs^.deinit_with(handler)`) vs comptime parameter
# (`xs^.deinit_with[handler]()`); `deinit` vs `var` element convention; a
# key/value pair vs a `DictEntry` for mappings; front-to-back vs
# back-to-front drain order (Mojito drains back-to-front via `List.pop`).
#
# Context: nightly §6. Mojito spells the family
# `deinit_with(deinit self, handler: def(deinit …) capturing[_])` on
# List/Array/Dict/Set/StringDict (kv-pair handlers for mappings) and keeps
# Tuple's comptime-parameter shape (`deinit_with[handler]()`, the
# `consume_elements` contract). `clear_with(mut self, handler)` on Dict/Set.
#
# Run:    mojo run deinit_with_handler_shape.mojo
#         cargo run -- run conformance/probes/deinit_with_handler_shape.mojo
#
# If the head uses comptime-parameter handlers, `var` conventions, an
#   entry-typed mapping handler, or front-to-back order: adjust the stdlib
#   signatures/bodies, assets/ok/container_owning_family_apis.mojo, and the
#   vm_test container_family_owning_apis_execute expectations.
def main():
    var xs: List[Int] = [1, 2]
    xs^.deinit_with(lambda (deinit element: Int): print("x", element))
    var d: Dict[Int, Int] = {1: 10, 2: 20}
    d.clear_with(lambda (deinit key: Int, deinit value: Int): print("kv", key, value))
