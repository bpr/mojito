# A type parameter inferred from a string literal binds `String`, the type
# the literal materializes to.
from std.reflection.type_info import _unqualified_type_name

def name_of[T: Movable & Deinitable](var x: T) -> String:
    return _unqualified_type_name[T]()

def main():
    print(name_of("x"))
    print(name_of(1))
