# expect: struct 'Point' has no field named 'z'
# A reflection query in a runtime position is answered at compile time, so
# an index query naming no field of its subject is rejected when the body is
# compiled, not when it runs.
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


def main():
    comptime r = reflect[Point]
    print(r.field_index["z"]())
