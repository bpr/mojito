# A field type selected at a `comptime for` index from a struct whose fields
# all have one type is that type, as upstream folds a selection over a
# homogeneous list: `types[i]` declares an `Int` and supports `+=`.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Int


def main():
    comptime types = reflect[P].field_types()
    comptime for i in range(reflect[P].field_count()):
        var v: types[i] = i
        v += 1
        print(v)
