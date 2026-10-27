# A `where` clause refines a type parameter for every member lookup, not
# only method calls: an associated value, an associated type, a projection's
# own bound, and an in-place operator a trait the clause names declares.


trait HasN:
    comptime N: Int
    comptime E: Copyable & Writable & Deinitable

    def get(self) -> Self.E:
        ...


trait Bump:
    def __iadd__(mut self, rhs: Self):
        ...


@fieldwise_init
struct S(Copyable, HasN):
    comptime N = 7
    comptime E = Int
    var x: Int

    def get(self) -> Self.E:
        return self.x


@fieldwise_init
struct C(Bump, Copyable, Writable):
    var x: Int

    def __iadd__(mut self, rhs: Self):
        self.x += rhs.x


def value_member[T: Copyable & Deinitable](s: T) where conforms_to(T, HasN):
    print(T.N)


def associated_type[T: Copyable & Deinitable](s: T) where conforms_to(T, HasN):
    var e: T.E = s.get()
    print(e)


def projection_bound[T: Copyable & Deinitable](s: T) where conforms_to(T, HasN):
    print(s.get())


def bump[T: Copyable & Writable](mut a: T, b: T) where conforms_to(T, Bump):
    a += b
    print(a)


def main():
    value_member(S(1))
    associated_type(S(2))
    projection_bound(S(3))
    var c = C(1)
    bump(c, C(2))
