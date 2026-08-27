# expect: has no field
# Field reads on a frozen struct fold at compile time and name real fields.
@fieldwise_init
struct Extent(Copyable, Movable):
    var rows: Int
    var cols: Int

comptime E = Extent(2, 3)
comptime BAD = E.missing

def main():
    print(BAD)
