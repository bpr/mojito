# `Tuple` and `Array` are `Defaultable` when every element type is: the
# nullary constructors default-construct each element (`Ts[i]()` /
# `Self.T()`), directly and through a `T: Defaultable` bound. Tuple's static
# `__len__()` coexists with the instance one, and the explicit
# `__contains__` dunder spelling works alongside `in`.
def make[T: Defaultable]() -> T:
    return T()

def main():
    var tuple = Tuple[Int, Bool]()
    print(tuple)
    var array = Array[Int, 3]()
    print(array[0], array[1], array[2])
    var made = make[Tuple[Int, Bool]]()
    print(made)
    var made_array = make[Array[Int, 2]]()
    print(made_array[0], made_array[1])
    var pair = Tuple(1, 2)
    print(Tuple[Int, Int].__len__(), pair.__len__(), len(pair))
    print(pair.__contains__(2), pair.__contains__(3), 2 in pair)
