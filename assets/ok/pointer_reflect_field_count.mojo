# A pointer's type is the application of the struct `std.memory.pointer`
# declares, so reflection reads that struct's one field, as upstream's does.
from std.reflection import reflect


def main():
    var x = 1
    var p = Pointer(to=x)
    print(p[], reflect[type_of(p)].field_count())
