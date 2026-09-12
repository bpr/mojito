# expect: is_same_type
# `is_same_type[T, U]()` was a Mojito-only predicate; upstream has no such
# declaration and compares type values with `==`
# (`assets/ok/type_predicate_comptime_if.mojo`). Neither a runtime `if` nor a
# `comptime if` resolves the name any more.
def name[T: AnyType]() -> String:
    if is_same_type[T, Int]():
        return "int"
    else:
        return "other"

def main():
    print(name[Int]())
