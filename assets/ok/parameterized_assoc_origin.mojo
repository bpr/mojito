# A parameterized associated type with an origin parameter (current Mojo's
# `IteratorType`) and its application `Self.IteratorType[origin_of(self)]` in a
# method return type. The trait declaration checks; the application's explicit
# (post-`//`) parameter arity is validated.
trait Iterator:
    comptime Element: Movable

trait Iterable:
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ]: Iterator

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        ...

def main():
    print(42)
