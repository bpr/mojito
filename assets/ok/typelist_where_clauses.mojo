# TypeList propositions in where clauses: a concrete `of` list folds
# eagerly in the checked constraint algebra. (The pack adapter
# `TypeList[Ts.values]()` lowers to the same pack constraint forms as the
# established `conforms_to(Ts.values, Trait)` vocabulary and shares its
# current variadic-def call limitations; the checker suite pins it.)
comptime IsSmall[T: AnyType] = IsTriviallyCopyable[T]

def checked_add(a: Int, b: Int) -> Int where TypeList.of[Trait=AnyType, Int, Bool]().contains[Int]():
    return a + b

def small_only(x: Int) -> Int where TypeList.of[Int, Bool]().all[IsSmall]():
    return x

def none_linear(x: Int) -> Int where not TypeList.of[Int]().any[IsTriviallyDeinitable]() or True:
    return x

def main():
    print(checked_add(2, 3))
    print(small_only(7))
    print(none_linear(9))
