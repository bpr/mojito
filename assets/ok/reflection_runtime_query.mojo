# A reflection query in a runtime position calls upstream's static
# `Reflected[T]` method, whose compile-time answer materializes where it
# stands: through a bound handle or spelled directly, in a free function or
# a method.
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


struct Holder:
    var count: Int

    def __init__(out self):
        self.count = reflect[Point].field_count()

    def second(self) -> Int:
        comptime r = reflect[Point]
        return r.field_index["y"]() + self.count


def main():
    comptime r = reflect[Point]
    print(r.field_count())
    print(reflect[Point].field_count(), reflect[Point].is_struct())
    var widened = r.field_count() + 1
    if r.is_struct():
        print("a struct of", widened - 1, "fields")
    print(r.field_index["x"](), reflect[Point].field_index["y"]())
    print(len(r.field_names()), r.field_names()[1])
    print(Holder().second())
