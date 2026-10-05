# expect: function instantiation of `missing_index` failed: struct 'Pair' has no field named 'z'
# A reflection query over a template's parameter naming a field the bound
# struct lacks fails the instantiation, as at the pin.
@fieldwise_init
struct Pair(Copyable):
    var left: Int
    var right: Int


def missing_index[T: AnyType]() -> Int:
    return reflect[T].field_index["z"]()


def main():
    print(missing_index[Pair]())
