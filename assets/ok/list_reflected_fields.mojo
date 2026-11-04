# `List` stores upstream's two fields: `_storage`, an `Allocation` whose
# `Layout` carries the capacity, and `_len`.
def main():
    comptime r = reflect[List[Int]]
    print(r.field_count())
    comptime for name in r.field_names():
        print(name)
    print(r.field_index["_len"]())
