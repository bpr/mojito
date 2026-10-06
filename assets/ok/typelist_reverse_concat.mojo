# A closed `TypeList` reverses and concatenates: the result measures in a
# runtime position and indexes in a `comptime if`.
def main():
    comptime tl = TypeList.of[Trait=AnyType, Int, String, Float64]()
    comptime r = tl.reverse()
    print(r.length)
    comptime if r[0] == Float64:
        print("reversed")
    comptime c = TypeList._concat[tl.values, r.values]()
    print(c.length)
