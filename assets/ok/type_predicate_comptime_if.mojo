# Type values compare with `==` in a comptime branch on a type parameter
# (Phase 7): name[Int] takes the `int` branch, name[String] the else.
def name[T: AnyType]() -> String:
    comptime if T == Int:
        return "int"
    else:
        return "other"

def main():
    print(name[Int]())
    print(name[String]())
