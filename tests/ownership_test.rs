//! Phase 4 — ownership (move) analysis tests.
//!
//! `check_ownership` runs after type-checking and models Mojo's move semantics: a
//! value transferred with `^` may not be used again. These tests cover the
//! positive cases (a move is fine if the value isn't used afterward, or is
//! reinitialized) and the violations (use-after-move, conditional move); the
//! file fixtures under `assets/ownership_error/` and `assets/ownership_ok/`
//! run per-file in `tests/corpus_test.rs` (`ownership_error::*` /
//! `ownership_ok::*`).

use mojito::{OwnershipError, check, check_ownership, elaborate, parse};

/// Elaborate and type-check `src` (the production stage order), then run the
/// ownership analysis.
fn own(src: &str) -> Result<(), OwnershipError> {
    let program = parse(src).expect("parse error");
    let program = elaborate(program).expect("comptime error");
    check(&program).expect("type error");
    check_ownership(&program)
}

#[test]
fn ownership_reports_invalid_unchecked_input_instead_of_panicking() {
    let program = parse("def main():\n    print(missing)\n").expect("parse error");
    assert!(matches!(
        check_ownership(&program),
        Err(OwnershipError::InvalidInput(message)) if message.contains("missing")
    ));
}

#[test]
fn pointer_loan_blocks_owner_access_while_live() {
    let src =
        "def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    x = 5\n    print(p[0])\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn pointer_loan_transfers_through_copies() {
    let src = "def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    var q = p\n    x = 5\n    print(q[0])\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn owner_move_while_pointer_live_is_rejected() {
    // The stable-pointer deref substitutes the owner place, so the post-move
    // access surfaces as a use-after-move on the owner; a handle-carried
    // pointer would surface the same invalidation as a loan conflict.
    let src = "@fieldwise_init\nstruct Cell:\n    var n: Int\n\ndef main():\n    var cell = Cell(1)\n    var p = UnsafePointer(to=cell.n)\n    var moved = cell^\n    print(p[0])\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::UseAfterMove { .. } | OwnershipError::LoanConflict { .. })
    ));
}

#[test]
fn pointer_aggregate_extends_the_owner_loan() {
    let src = "@fieldwise_init\nstruct Borrowed[origin: Origin]:\n    var ptr: UnsafePointer[Int, Self.origin]\n\ndef main():\n    var value = 40\n    var b = Borrowed(UnsafePointer(to=value))\n    value += 1\n    print(b.ptr[0])\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn dead_pointer_releases_its_owner_loan() {
    let src = "def main():\n    var x = 1\n    var p = UnsafePointer(to=x)\n    p[0] = 2\n    x = 5\n    print(x)\n";
    assert!(own(src).is_ok());
}

#[test]
fn reference_aggregate_extends_the_owner_loan() {
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n\ndef main():\n    var value = 40\n    ref alias = value\n    var box = RefBox(alias)\n    value += 1\n    print(box.value)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn reference_aggregate_preserves_every_field_loan() {
    let src = "@fieldwise_init\nstruct RefPair[a: Origin[mut=True], b: Origin[mut=True]]:\n    var first: ref[a] Int\n    var second: ref[b] Int\n\ndef main():\n    var x = 10\n    var y = 20\n    ref rx = x\n    ref ry = y\n    var pair = RefPair(rx, ry)\n    y += 1\n    print(pair.second)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn moving_reference_aggregate_transfers_its_owner_loan() {
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n\ndef main():\n    var value = 40\n    ref alias = value\n    var box = RefBox(alias)\n    var moved = box^\n    value += 1\n    print(moved.value)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn rebinding_reference_aggregate_replaces_its_owner_generation() {
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] Int\n\ndef main():\n    var x = 1\n    var y = 10\n    ref rx = x\n    ref ry = y\n    var box = RefBox(rx)\n    print(box.value)\n    box = RefBox(ry)\n    x = 2\n    print(box.value)\n";
    assert!(own(src).is_ok());
}

#[test]
fn nested_reference_aggregate_preserves_every_element_loan() {
    // Executable `ref` fields are a Mojito extension used to prove the checked
    // aggregate/loan representation; current Mojo spells stored provenance with
    // origin-bearing pointer types instead.
    let src = "@fieldwise_init\nstruct RefTuple[origin: Origin[mut=True]]:\n    var values: Tuple[ref[origin] Int, ref[origin] Int]\n\ndef main():\n    var x = 10\n    var y = 20\n    ref rx = x\n    ref ry = y\n    var pair = RefTuple((rx, ry))\n    y += 1\n    print(pair.values[1])\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));

    let src = "@fieldwise_init\nstruct RefList[origin: Origin[mut=True]]:\n    var values: List[ref[origin] Int]\n\ndef main():\n    var x = 10\n    var y = 20\n    ref rx = x\n    ref ry = y\n    var pair = RefList([rx, ry])\n    y += 1\n    print(pair.values[1])\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn variant_payload_reference_is_invalidated_when_the_tag_changes() {
    // The ownership unit runs the unlinked checker, so a local declaration makes
    // the compiler-provided Variant name visible without involving module I/O.
    let src = "struct Variant:\n    pass\n\ndef main():\n    var value = Variant[Int, String](7)\n    ref payload = value[Int]\n    value.set[String](\"changed\")\n    print(payload)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { origin, .. })
            if origin == "value[\"value\"]"
    ));
}

#[test]
fn list_interior_references_allow_reads_direct_writes_and_overlap() {
    let src = "def main():\n    var values = [10, 20, 30]\n    ref first = values[0]\n    ref same = values[0]\n    print(len(values))\n    same += 1\n    values[0] = 77\n    print(first, same)\n";
    assert!(own(src).is_ok());
}

#[test]
fn a_reborrow_derives_permission_from_its_parent_reference() {
    let src = "def main():\n    var value = 40\n    ref first = value\n    ref second = first\n    second += 2\n    print(value)\n";
    assert!(own(src).is_ok());
}

#[test]
fn structural_list_mutation_invalidates_an_old_element_generation() {
    let src = "def main():\n    var values = [10, 20, 30]\n    ref first = values[0]\n    values.append(40)\n    print(first)\n";
    match own(src) {
        Err(OwnershipError::InvalidatedInteriorReference {
            reference,
            origin,
            span,
            invalidated_at,
        }) => {
            assert_eq!(reference, "first");
            assert_eq!(origin, "values[\"element\"]");
            assert!(src[span.span.0..span.span.1].contains("first"));
            assert!(src[invalidated_at.span.0..invalidated_at.span.1].contains("append"));
        }
        other => panic!("expected stale List element generation, got {other:?}"),
    }
}

#[test]
fn structural_list_mutation_invalidates_a_borrowed_iterator_generation() {
    let source = "def main():\n    var values = [1, 2, 3, 4]\n    for value in values:\n        print(value)\n        if value == 1:\n            values.append(5)\n";
    assert!(matches!(
        own(source),
        Err(OwnershipError::InvalidatedInteriorReference { reference, origin, .. })
            if reference.starts_with("$iter") && origin == "values[\"element\"]"
    ));

    let no_later_use = "def main():\n    var values = [1, 2, 3, 4]\n    for value in values:\n        values.append(5)\n        break\n";
    assert!(
        own(no_later_use).is_ok(),
        "an invalidated iterator may be discarded without reading through it"
    );
}

#[test]
fn replacing_a_collection_invalidates_its_old_interiors() {
    let src = "def main():\n    var values = [10, 20, 30]\n    ref first = values[0]\n    values = [40, 50]\n    print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn whole_replacement_through_reference_invalidates_nested_interiors() {
    let src = "def main():\n    var values = [1, 2]\n    ref whole = values\n    ref first = values[0]\n    whole = [3, 4]\n    print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn unpack_replacement_invalidates_old_collection_interiors() {
    let src = "def main():\n    var values = [10, 20, 30]\n    var count = 0\n    ref first = values[0]\n    values, count = ([40, 50], 2)\n    print(first, count)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn indexed_and_variant_payload_replacement_invalidate_nested_interiors() {
    let indexed = "def main():\n    var outer = [[1, 2]]\n    ref first = outer[0][0]\n    outer[0] = [3, 4]\n    print(first)\n";
    assert!(matches!(
        own(indexed),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));

    let unpacked = "def main():\n    var outer = [[1, 2]]\n    var count = 0\n    ref first = outer[0][0]\n    outer[0], count = ([3, 4], 2)\n    print(first, count)\n";
    assert!(matches!(
        own(unpacked),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));

    let variant = "struct Variant:\n    pass\n\ndef main():\n    var value = Variant[List[Int], String]([1, 2])\n    ref first = value[List[Int]][0]\n    value[List[Int]] = [3, 4]\n    print(first)\n";
    assert!(matches!(
        own(variant),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn pointer_whole_replacement_invalidates_referent_interiors() {
    let src = "def main():\n    var values = [1, 2]\n    ref first = values[0]\n    var pointer = UnsafePointer(to=values)\n    pointer[0] = [3, 4]\n    print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn reference_field_write_invalidates_the_referents_interiors() {
    let src = "@fieldwise_init\nstruct RefBox[origin: Origin[mut=True]]:\n    var value: ref[origin] List[Int]\n\ndef main():\n    var values = [1, 2]\n    ref whole = values\n    var box = RefBox(whole)\n    ref first = box.value[0]\n    box.value = [3, 4]\n    print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn reference_field_write_is_field_sensitive() {
    let distinct = "@fieldwise_init\nstruct RefPair[left_origin: Origin[mut=True], right_origin: Origin[mut=True]]:\n    var left: ref[left_origin] List[Int]\n    var right: ref[right_origin] List[Int]\n\ndef main():\n    var left = [1, 2]\n    var right = [3, 4]\n    ref left_ref = left\n    ref right_ref = right\n    var pair = RefPair(left_ref, right_ref)\n    ref right_element = pair.right[0]\n    pair.left = [5, 6]\n    print(right_element)\n";
    assert!(own(distinct).is_ok());

    let same = "@fieldwise_init\nstruct RefPair[left_origin: Origin[mut=True], right_origin: Origin[mut=True]]:\n    var left: ref[left_origin] List[Int]\n    var right: ref[right_origin] List[Int]\n\ndef main():\n    var left = [1, 2]\n    var right = [3, 4]\n    ref left_ref = left\n    ref right_ref = right\n    var pair = RefPair(left_ref, right_ref)\n    ref right_element = pair.right[0]\n    pair.right = [5, 6]\n    print(right_element)\n";
    assert!(matches!(
        own(same),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn handwritten_reference_fields_keep_their_referent_identity() {
    let src = "struct RefPair[left_origin: Origin[mut=True], right_origin: Origin[mut=True]]:\n    var left: ref[left_origin] List[Int]\n    var right: ref[right_origin] List[Int]\n    def __init__(out self, ref[left_origin] left: List[Int], ref[right_origin] right: List[Int]):\n        self.left = left\n        self.right = right\n\ndef main():\n    var left = [1, 2]\n    var right = [3, 4]\n    var pair = RefPair(left, right)\n    ref right_element = pair.right[0]\n    pair.right = [5, 6]\n    print(right_element)\n";
    let result = own(src);
    assert!(
        matches!(
            result,
            Err(OwnershipError::InvalidatedInteriorReference { .. })
        ),
        "{result:?}"
    );
}

#[test]
fn list_interior_invalidation_joins_across_control_flow() {
    let src = "def main():\n    var values = [10, 20, 30]\n    ref first = values[0]\n    if Bool(1):\n        values.append(40)\n    print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn fresh_list_element_generation_after_mutation_is_valid() {
    let src = "def main():\n    var values = [10, 20, 30]\n    ref old = values[0]\n    print(old)\n    values.append(40)\n    ref fresh = values[3]\n    print(fresh)\n";
    assert!(own(src).is_ok());
}

#[test]
fn interior_invalidation_is_field_sensitive() {
    let src = "@fieldwise_init\nstruct Pair:\n    var left: List[Int]\n    var right: List[Int]\n\ndef main():\n    var pair = Pair([1], [2])\n    ref right = pair.right[0]\n    pair.left.append(3)\n    print(right)\n";
    assert!(own(src).is_ok());

    let src = "@fieldwise_init\nstruct Pair:\n    var left: List[Int]\n    var right: List[Int]\n\ndef main():\n    var pair = Pair([1], [2])\n    ref left = pair.left[0]\n    pair.left.append(3)\n    print(left)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn mutable_cross_call_invalidates_collection_interiors() {
    let src = "def alter(mut values: List[Int]):\n    values.append(4)\n\ndef main():\n    var values = [1, 2, 3]\n    ref first = values[0]\n    alter(values)\n    print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn copied_interior_argument_is_read_before_mutable_call_invalidation() {
    let src = "def alter(mut values: List[Int], copied: Int):\n    values.append(copied)\n\ndef main():\n    var values = [1, 2]\n    ref first = values[0]\n    alter(values, first)\n";
    assert!(own(src).is_ok());
}

#[test]
fn explicit_interior_return_contract_preserves_the_full_receiver_path() {
    let src = "@fieldwise_init\nstruct Bucket:\n    var values: List[Int]\n    def at(ref self, index: Int) -> ref[origin_of(self.values)._get_owned_interior[\"element\"]] Int:\n        return self.values[index]\n\ndef main():\n    var bucket = Bucket([1, 2, 3])\n    ref first = bucket.at(0)\n    bucket.values.append(4)\n    print(first)\n";
    let result = own(src);
    assert!(
        matches!(
            &result,
            Err(OwnershipError::InvalidatedInteriorReference { origin, .. })
                if origin == "bucket.values[\"element\"]"
        ),
        "got {result:?}"
    );
}

#[test]
fn union_interior_return_allows_owner_reads_but_tracks_every_member() {
    let valid = "def choose(ref left: List[Int], ref right: List[Int], flag: Bool) -> ref[origin_of(left)._get_owned_interior[\"element\"], origin_of(right)._get_owned_interior[\"element\"]] Int:\n    if flag:\n        return left[0]\n    return right[0]\n\ndef main():\n    var left = [1]\n    var right = [2]\n    ref selected = choose(left, right, True)\n    print(len(left), len(right))\n    print(selected)\n";
    assert!(own(valid).is_ok());

    let invalid = "def choose(ref left: List[Int], ref right: List[Int], flag: Bool) -> ref[origin_of(left)._get_owned_interior[\"element\"], origin_of(right)._get_owned_interior[\"element\"]] Int:\n    if flag:\n        return left[0]\n    return right[0]\n\ndef main():\n    var left = [1]\n    var right = [2]\n    ref selected = choose(left, right, True)\n    right.append(3)\n    print(selected)\n";
    assert!(matches!(
        own(invalid),
        Err(OwnershipError::InvalidatedInteriorReference { origin, .. })
            if origin == "right[\"element\"]"
    ));
}

#[test]
fn forwarding_a_union_reference_preserves_every_interior_generation() {
    let src = "def element(ref values: List[Int]) -> ref[origin_of(values)._get_owned_interior[\"element\"]] Int:\n    return values[0]\n\ndef choose(ref left: Int, ref right: Int, flag: Bool) -> ref[left, right] Int:\n    if flag:\n        return left\n    else:\n        return right\n\ndef forward(ref value: Int) -> ref[value] Int:\n    return value\n\ndef main():\n    var left_values = [1, 10]\n    var right_values = [2, 20]\n    ref left = element(left_values)\n    ref right = element(right_values)\n    ref selected = choose(left, right, False)\n    ref forwarded = forward(selected)\n    right_values.append(3)\n    print(forwarded)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { origin, .. })
            if origin == "right_values[\"element\"]"
    ));
}

#[test]
fn mixed_union_return_keeps_its_ordinary_owner_loan() {
    let src = "def choose(ref plain: Int, ref values: List[Int], flag: Bool) -> ref[plain, origin_of(values)._get_owned_interior[\"element\"]] Int:\n    if flag:\n        return values[0]\n    return plain\n\ndef main():\n    var plain = 1\n    var values = [2]\n    ref selected = choose(plain, values, False)\n    plain = 9\n    print(selected)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn immutable_returned_reference_remains_a_shared_loan() {
    let src = "def borrow[origin: Origin[mut=False]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef main():\n    var value = 1\n    ref alias = borrow(value)\n    print(value)\n    print(alias)\n";
    assert!(own(src).is_ok());
}

#[test]
fn read_only_subscript_contract_coexists_with_a_shared_field_loan() {
    let src = "def borrow[origin: Origin[mut=False]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\n@fieldwise_init\nstruct Box(Copyable, Movable):\n    var value: Int\n    def __getitem__(self, index: Int) -> Int:\n        return self.value + index\n\ndef main():\n    var box = Box(40)\n    ref alias = borrow(box.value)\n    print(box[2])\n    print(alias)\n";
    assert!(own(src).is_ok());
}

#[test]
fn mutable_subscript_receiver_conflicts_with_a_live_shared_field_loan() {
    let src = "def borrow[origin: Origin[mut=False]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\n@fieldwise_init\nstruct Box(Copyable, Movable):\n    var value: Int\n    def __getitem__(mut self, index: Int) -> Int:\n        self.value += index\n        return self.value\n\ndef main():\n    var box = Box(40)\n    ref alias = borrow(box.value)\n    print(box[2])\n    print(alias)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn immutable_ref_subscript_argument_retains_a_read_place() {
    let src = "def borrow[origin: Origin[mut=False]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\n@fieldwise_init\nstruct Lookup(Copyable, Movable):\n    var offset: Int\n    def __getitem__[origin: Origin[mut=False]](\n        self, ref[origin] index: Int\n    ) -> Int:\n        return self.offset + index\n\ndef main():\n    var position = 2\n    ref alias = borrow(position)\n    var lookup = Lookup(40)\n    print(lookup[position])\n    print(alias)\n";
    assert!(own(src).is_ok());
}

#[test]
fn dict_lookup_defines_a_new_value_generation() {
    let src = "def main():\n    var values = {\"a\": 10}\n    ref first = values[\"a\"]\n    print(values[\"a\"])\n    print(first)\n";
    let result = own(src);
    assert!(
        matches!(
            &result,
            Err(OwnershipError::InvalidatedInteriorReference { origin, .. })
                if origin == "values[\"value\"]"
        ),
        "got {result:?}"
    );
}

#[test]
fn dict_lookup_refresh_invalidates_fields_of_the_previous_value_generation() {
    let src = "@fieldwise_init\nstruct Item(Copyable, Movable):\n    var value: Int\n\ndef main():\n    var values = {\"a\": Item(10)}\n    ref field = values[\"a\"].value\n    print(values[\"a\"].value)\n    print(field)\n";
    let result = own(src);
    assert!(
        matches!(
            &result,
            Err(OwnershipError::InvalidatedInteriorReference { origin, .. })
                if origin == "values[\"value\"].value"
        ),
        "got {result:?}"
    );
}

#[test]
fn retained_subscript_reference_is_checked_after_later_argument_evaluation() {
    let src = "def mutate(mut values: List[Int]) -> Int:\n    values.append(3)\n    return 0\n\n@fieldwise_init\nstruct Reader:\n    var marker: Int\n    def __getitem__[origin: Origin[mut=False]](\n        self, ref[origin] first: Int, second: Int\n    ) -> Int:\n        return first + second\n\ndef main():\n    var values = [10, 20]\n    ref first = values[0]\n    var reader = Reader(0)\n    print(reader[first, mutate(values)])\n";
    let result = own(src);
    assert!(
        matches!(
            &result,
            Err(OwnershipError::InvalidatedInteriorReference { reference, origin, .. })
                if reference == "first" && origin == "values[\"element\"]"
        ),
        "got {result:?}"
    );
}

#[test]
fn copied_subscript_argument_is_not_reread_after_later_argument_evaluation() {
    let src = "def mutate(mut values: List[Int]) -> Int:\n    values.append(3)\n    return 0\n\n@fieldwise_init\nstruct Reader:\n    var marker: Int\n    def __getitem__(self, first: Int, second: Int) -> Int:\n        return first + second\n\ndef main():\n    var values = [10, 20]\n    ref first = values[0]\n    var reader = Reader(0)\n    print(reader[first, mutate(values)])\n";
    assert!(own(src).is_ok());
}

#[test]
fn raising_mutation_invalidates_on_the_try_path() {
    let src = "def alter(mut values: List[Int]) raises:\n    values.append(4)\n    raise Error(\"changed\")\n\ndef main():\n    var values = [1, 2, 3]\n    ref first = values[0]\n    try:\n        alter(values)\n    except error:\n        pass\n    print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn returning_try_path_does_not_invalidate_the_fallthrough_path() {
    let src = "def main():\n    var values = [1, 2, 3]\n    ref first = values[0]\n    try:\n        if Bool(1):\n            values.append(4)\n            return\n    finally:\n        pass\n    print(first)\n";
    assert!(own(src).is_ok());
}

#[test]
fn handler_entry_excludes_nonraising_mutations_after_an_earlier_failure() {
    let src = "def fail() raises:\n    raise Error(\"stop\")\n\ndef main():\n    var values = [1, 2, 3]\n    ref first = values[0]\n    try:\n        fail()\n        values.append(4)\n    except error:\n        print(first)\n";
    assert!(own(src).is_ok());
}

#[test]
fn finally_checks_the_returning_path_before_it_exits() {
    let src = "def main():\n    var values = [1, 2, 3]\n    ref first = values[0]\n    try:\n        values.append(4)\n        return\n    finally:\n        print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

#[test]
fn nested_try_propagates_invalidated_state_to_an_outer_handler() {
    let src = "def alter(mut values: List[Int]) raises:\n    values.append(4)\n    raise Error(\"changed\")\n\ndef main():\n    var values = [1, 2, 3]\n    ref first = values[0]\n    try:\n        try:\n            alter(values)\n        finally:\n            pass\n    except error:\n        print(first)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::InvalidatedInteriorReference { .. })
    ));
}

const THING: &str = "@fieldwise_init\nstruct Thing:\n    var x: Int\n\n";

#[test]
fn move_without_later_use_is_ok() {
    // Transferring a value is fine as long as the source isn't used afterward.
    let src = format!(
        "{THING}def main():\n    var a: Thing = Thing(1)\n    var b: Thing = a^\n    print(b.x)\n"
    );
    assert!(own(&src).is_ok());
}

#[test]
fn move_capture_consumes_the_source_at_the_nested_declaration() {
    let moved = "def main():\n    var values = [40]\n    def take() {var values^} -> Int:\n        return values[0]\n    print(values[0])\n";
    assert!(matches!(
        own(moved),
        Err(OwnershipError::UseAfterMove { .. })
    ));

    let no_later_use = "def main():\n    var values = [40]\n    def take() {var values^} -> Int:\n        return values[0]\n    print(take())\n";
    assert!(own(no_later_use).is_ok());
}

#[test]
fn unused_explicit_move_capture_still_consumes_the_source() {
    let source = "def main():\n    var values = [40]\n    def take() {var values^}:\n        pass\n    print(values[0])\n";
    assert!(matches!(
        own(source),
        Err(OwnershipError::UseAfterMove { .. })
    ));
}

#[test]
fn owned_iteration_consumes_the_source_collection() {
    let ok_source = format!(
        "{THING}def main():\n    var values = [Thing(1), Thing(2)]\n    for var item in values^:\n        print(item.x)\n"
    );
    assert!(own(&ok_source).is_ok());

    let used_again = format!(
        "{THING}def main():\n    var values = [Thing(1), Thing(2)]\n    for var item in values^:\n        print(item.x)\n    print(len(values))\n"
    );
    match own(&used_again) {
        Err(OwnershipError::UseAfterMove { .. }) => {}
        other => panic!("expected owned iteration to consume its source, got {other:?}"),
    }
}

#[test]
fn reassign_after_move_is_ok() {
    // A moved variable reinitialized before its next use is fine ("reinit").
    let src = format!(
        "{THING}def main():\n    var a: Thing = Thing(1)\n    var b: Thing = a^\n    a = Thing(2)\n    print(a.x)\n"
    );
    assert!(own(&src).is_ok());
}

#[test]
fn no_transfer_never_errors() {
    // Without `^`, nothing moves; ordinary value semantics are untouched.
    let src = "def main():\n    var a: Int = 1\n    var b: Int = a\n    print(a)\n    print(b)\n";
    assert!(own(src).is_ok());
}

#[test]
fn transferred_tuple_reverse_consumes_the_receiver() {
    let src = "def main():\n    var pair = Tuple(3, \"seven\")\n    var reversed = pair^.reverse()\n    print(reversed)\n    print(pair)\n";
    assert!(matches!(own(src), Err(OwnershipError::UseAfterMove { .. })));
}

#[test]
fn transferred_tuple_concat_consumes_both_operands() {
    let src = "def main():\n    var left = Tuple(3, \"seven\")\n    var right = Tuple(True)\n    var joined = left^.concat(right^)\n    print(joined)\n    print(right)\n";
    assert!(matches!(own(src), Err(OwnershipError::UseAfterMove { .. })));
}

#[test]
fn local_reference_loan_ends_at_last_use() {
    let src = "def main():\n    var value: Int = 1\n    ref alias = value\n    print(alias)\n    value = 2\n    print(value)\n";
    assert!(own(src).is_ok());
}

#[test]
fn local_reference_blocks_owner_access_while_live() {
    let src = "def main():\n    var value: Int = 1\n    ref alias = value\n    value = 2\n    print(alias)\n";
    assert!(matches!(
        own(src),
        Err(OwnershipError::LoanConflict { place, loan, .. })
            if place == "value" && loan == "alias"
    ));
}

#[test]
fn local_reference_loans_are_field_sensitive() {
    let src = "@fieldwise_init\nstruct Pair:\n    var left: Int\n    var right: Int\n\ndef main():\n    var pair = Pair(1, 2)\n    ref alias = pair.left\n    pair.right = 3\n    print(alias)\n";
    assert!(own(src).is_ok());
}

#[test]
fn local_reference_loan_flows_through_cfg_join() {
    let src = "def main():\n    var value: Int = 1\n    var flag: Bool = True\n    ref alias = value\n    if flag:\n        print(0)\n    value = 2\n    print(alias)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn local_reference_blocks_mutating_calls_between_uses() {
    let src = "def replace(mut value: Int):\n    value = 2\n\ndef main():\n    var value: Int = 1\n    ref alias = value\n    replace(value)\n    print(alias)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}

#[test]
fn callable_capture_effects_conflict_with_live_reference_loans() {
    let direct = "def main():\n    var value = 1\n    def replace() {mut value}:\n        value = 2\n    ref alias = value\n    replace()\n    print(alias)\n";
    assert!(matches!(
        own(direct),
        Err(OwnershipError::LoanConflict { place, loan, .. })
            if place == "value" && loan == "alias"
    ));

    // The same concrete environment access must cross an indirect downward
    // funarg boundary. Merely checking the ordinary `invoke` argument place
    // would miss the write performed later through `callback()`.
    let indirect = "def invoke(callback: def()):\n    callback()\n\ndef main():\n    var value = 1\n    def replace() {mut value}:\n        value = 2\n    ref alias = value\n    invoke(replace)\n    print(alias)\n";
    assert!(matches!(
        own(indirect),
        Err(OwnershipError::LoanConflict { place, loan, .. })
            if place == "value" && loan == "alias"
    ));
}

#[test]
fn callable_capture_effects_end_at_the_call_boundary() {
    let source = "def main():\n    var value = 1\n    def replace() {mut value}:\n        value = 2\n    ref alias = value\n    print(alias)\n    replace()\n    print(value)\n";
    assert!(own(source).is_ok());
}

#[test]
fn returned_reference_establishes_a_persistent_caller_loan() {
    let source = "def borrow(ref value: Int) -> ref[value] Int:\n    return value\n\ndef main():\n    var value = 1\n    ref alias = borrow(value)\n    value = 2\n    print(alias)\n";
    assert!(matches!(
        own(source),
        Err(OwnershipError::LoanConflict { .. })
    ));

    let after_last_use = "def borrow(ref value: Int) -> ref[value] Int:\n    return value\n\ndef main():\n    var value = 1\n    ref alias = borrow(value)\n    print(alias)\n    value = 2\n    print(value)\n";
    assert!(own(after_last_use).is_ok());
}

#[test]
fn use_after_move_is_rejected() {
    let src = format!(
        "{THING}def main():\n    var a: Thing = Thing(1)\n    var b: Thing = a^\n    print(a.x)\n"
    );
    match own(&src) {
        Err(OwnershipError::UseAfterMove { var, span }) => {
            assert_eq!(var, "a");
            // The message names the moved variable `a`; the span points at the
            // offending use expression `a.x` in `print(a.x)`.
            assert_eq!(&src[span.span.0..span.span.1], "a.x");
        }
        other => panic!("expected UseAfterMove, got {other:?}"),
    }
}

#[test]
fn double_transfer_is_use_after_move() {
    let src = format!(
        "{THING}def main():\n    var a: Thing = Thing(1)\n    var b: Thing = a^\n    var c: Thing = a^\n    print(b.x)\n"
    );
    assert!(matches!(
        own(&src),
        Err(OwnershipError::UseAfterMove { .. })
    ));
}

#[test]
fn conditional_move_is_rejected() {
    // Moved on one branch of an `if`, then used after the merge.
    let src = format!(
        "{THING}def main():\n    var flag: Bool = True\n    var a: Thing = Thing(1)\n    if flag:\n        var b: Thing = a^\n    print(a.x)\n"
    );
    match own(&src) {
        Err(OwnershipError::ConditionallyMoved { var, .. }) => assert_eq!(var, "a"),
        other => panic!("expected ConditionallyMoved, got {other:?}"),
    }
}

#[test]
fn move_in_loop_is_rejected() {
    // The first iteration moves `a`; the back-edge makes it (maybe-)moved on entry
    // to the second, so the transfer is flagged.
    let src = format!(
        "{THING}def main():\n    var a: Thing = Thing(1)\n    for i in range(3):\n        var b: Thing = a^\n        print(b.x)\n"
    );
    assert!(own(&src).is_err());
}

#[test]
fn use_after_move_through_a_place_write() {
    // Writing a field of a moved value is a use-after-move (caught via the place
    // root, not just a plain read).
    let src = format!(
        "{THING}def main():\n    var a: Thing = Thing(1)\n    var b: Thing = a^\n    a.x = 5\n"
    );
    assert!(matches!(
        own(&src),
        Err(OwnershipError::UseAfterMove { .. })
    ));
}

// Two non-copyable struct fields, so a partial move of one is unambiguous.
const PAIR: &str = "@fieldwise_init\nstruct Inner:\n    var id: Int\n\n@fieldwise_init\nstruct Pair:\n    var a: Inner\n    var b: Inner\n\n";

#[test]
fn partial_move_leaves_sibling_usable() {
    // Moving `p.a` out leaves `p.b` initialized and usable — field-sensitivity.
    let src = format!(
        "{PAIR}def main():\n    var p: Pair = Pair(Inner(1), Inner(2))\n    var x: Inner = p.a^\n    print(x.id)\n    print(p.b.id)\n"
    );
    assert!(
        own(&src).is_ok(),
        "sibling use after partial move: {:?}",
        own(&src)
    );
}

#[test]
fn use_of_moved_field_is_rejected() {
    // Reading the moved-out field itself is a use-after-move, named at the field.
    let src = format!(
        "{PAIR}def main():\n    var p: Pair = Pair(Inner(1), Inner(2))\n    var x: Inner = p.a^\n    print(p.a.id)\n"
    );
    match own(&src) {
        Err(OwnershipError::UseAfterMove { var, .. }) => assert_eq!(var, "p.a"),
        other => panic!("expected UseAfterMove of p.a, got {other:?}"),
    }
}

#[test]
fn whole_use_after_partial_move_is_rejected() {
    // Using the whole `p` (here transferring it) after a field was moved out is a
    // use-after-move, blamed on the moved field.
    let src = format!(
        "{PAIR}def main():\n    var p: Pair = Pair(Inner(1), Inner(2))\n    var x: Inner = p.a^\n    var q: Pair = p^\n    print(q.b.id)\n"
    );
    match own(&src) {
        Err(OwnershipError::UseAfterMove { var, .. }) => assert_eq!(var, "p.a"),
        other => panic!("expected UseAfterMove blamed on p.a, got {other:?}"),
    }
}

#[test]
fn reinitializing_a_moved_field_is_ok() {
    // Assigning the moved field re-initializes it; the whole value is usable again.
    let src = format!(
        "{PAIR}def main():\n    var p: Pair = Pair(Inner(1), Inner(2))\n    var x: Inner = p.a^\n    p.a = Inner(9)\n    print(p.a.id)\n    var q: Pair = p^\n    print(q.a.id)\n"
    );
    assert!(
        own(&src).is_ok(),
        "reinit after partial move: {:?}",
        own(&src)
    );
}

#[test]
fn conditional_partial_move_is_rejected() {
    // Moving `p.a` on one `if` arm makes it maybe-moved after the merge.
    let src = format!(
        "{PAIR}def main():\n    var flag: Bool = True\n    var p: Pair = Pair(Inner(1), Inner(2))\n    if flag:\n        var x: Inner = p.a^\n    print(p.a.id)\n"
    );
    match own(&src) {
        Err(OwnershipError::ConditionallyMoved { var, .. }) => assert_eq!(var, "p.a"),
        other => panic!("expected ConditionallyMoved of p.a, got {other:?}"),
    }
}

#[test]
fn moving_a_field_twice_is_use_after_move() {
    let src = format!(
        "{PAIR}def main():\n    var p: Pair = Pair(Inner(1), Inner(2))\n    var x: Inner = p.a^\n    var y: Inner = p.a^\n    print(x.id)\n    print(y.id)\n"
    );
    assert!(matches!(
        own(&src),
        Err(OwnershipError::UseAfterMove { .. })
    ));
}

#[test]
fn use_after_move_through_a_method_call() {
    let src = "@fieldwise_init\nstruct Thing:\n    var x: Int\n    def get(self) -> Int:\n        return self.x\n\ndef main():\n    var a: Thing = Thing(1)\n    var b: Thing = a^\n    print(a.get())\n";
    assert!(matches!(own(src), Err(OwnershipError::UseAfterMove { .. })));
}

#[test]
fn immutable_yield_iteration_still_conflicts_with_source_mutation() {
    // The immutable-origin cast changes only the yielded capability; the
    // iterator's source loan is unchanged, so structural mutation during
    // iteration still conflicts (generation protection is loan-based, not
    // mutability-based).
    let src = "@fieldwise_init\nstruct StopIteration:\n    pass\n\n@fieldwise_init\nstruct NumbersIter[m: Bool, //, o: Origin[mut=m]]:\n    var src: ref[o] List[Int]\n    var index: Int\n    def __next__(mut self) raises StopIteration -> ref[Origin[mut=False].cast_from[o]] Int:\n        if self.index >= len(self.src):\n            raise StopIteration()\n        var r = self.index\n        self.index += 1\n        return self.src[r]\n\nstruct Numbers:\n    var items: List[Int]\n    def __init__(out self):\n        self.items = [4, 5, 6]\n    def __iter__(ref self) -> NumbersIter:\n        ref items = self.items\n        return NumbersIter(items, 0)\n\ndef main():\n    var nums = Numbers()\n    for ref x in nums:\n        nums.items.append(7)\n        print(x)\n";
    assert!(matches!(own(src), Err(OwnershipError::LoanConflict { .. })));
}
