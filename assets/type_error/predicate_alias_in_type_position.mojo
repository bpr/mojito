# expect: Bool-valued comptime alias, not a type
# A predicate alias is a proposition; type positions reject it.
comptime IsCopy[T: AnyType] = conforms_to(T, Copyable)

def main():
    var x: IsCopy[Int] = 1
    print(x)
