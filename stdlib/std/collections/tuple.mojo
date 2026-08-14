# Public heterogeneous tuple.  `__RuntimeTuple` is a compiler-private pack
# aggregate: users see this nominal struct, while the private field gives the
# compiler exact per-index storage and move tracking.

struct Tuple[*Ts: Movable](
    Comparable where conforms_to(Ts.values, Comparable) and conforms_to(Ts.values, Equatable),
    Copyable where conforms_to(Ts.values, Copyable),
    Equatable where conforms_to(Ts.values, Equatable),
    ImplicitlyCopyable where conforms_to(Ts.values, ImplicitlyCopyable),
    Deinitable where conforms_to(Ts.values, Deinitable),
    Movable,
    Writable where conforms_to(Ts.values, Writable),
):
    comptime element_types = Ts
    var storage: __RuntimeTuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = __RuntimeTuple(*args^)

    # Current Mojo's compile-time-index hook.
    def __getitem_param__[index: Int](
        ref self
    ) -> ref[origin_of(self)] Ts[index]:
        return self.storage[index]

    def __len__(self) -> Int:
        return len(self.storage)

    def __eq__(self, other: Self) -> Bool where conforms_to(
        Ts.values, Equatable
    ):
        comptime for i in range(len(Ts)):
            if self.storage[i] != other.storage[i]:
                return False
        return True

    def __ne__(self, other: Self) -> Bool where conforms_to(
        Ts.values, Equatable
    ):
        comptime for i in range(len(Ts)):
            if self.storage[i] != other.storage[i]:
                return True
        return False

    def __lt__(self, other: Self) -> Bool where conforms_to(
        Ts.values, Comparable
    ) and conforms_to(Ts.values, Equatable):
        comptime for i in range(len(Ts)):
            if self.storage[i] < other.storage[i]:
                return True
            if other.storage[i] < self.storage[i]:
                return False
        return False

    def __le__(self, other: Self) -> Bool where conforms_to(
        Ts.values, Comparable
    ) and conforms_to(Ts.values, Equatable):
        comptime for i in range(len(Ts)):
            if self.storage[i] < other.storage[i]:
                return True
            if other.storage[i] < self.storage[i]:
                return False
        return True

    def __gt__(self, other: Self) -> Bool where conforms_to(
        Ts.values, Comparable
    ) and conforms_to(Ts.values, Equatable):
        comptime for i in range(len(Ts)):
            if other.storage[i] < self.storage[i]:
                return True
            if self.storage[i] < other.storage[i]:
                return False
        return False

    def __ge__(self, other: Self) -> Bool where conforms_to(
        Ts.values, Comparable
    ) and conforms_to(Ts.values, Equatable):
        comptime for i in range(len(Ts)):
            if other.storage[i] < self.storage[i]:
                return True
            if self.storage[i] < other.storage[i]:
                return False
        return True

    def __contains__[T: Equatable & Copyable & Movable](
        self, value: T
    ) -> Bool:
        comptime for i in range(len(Ts)):
            comptime if is_same_type[T, Ts[i]]():
                if self.storage[i] == value:
                    return True
        return False

    def write_to(self, mut writer: Some[Writer]) where conforms_to(
        Ts.values, Writable
    ):
        writer.write("(")
        comptime for i in range(len(Ts)):
            comptime if i > 0:
                writer.write(", ")
            writer.write(self.storage[i])
        comptime if len(Ts) == 1:
            writer.write(",")
        writer.write(")")

    def consume_elements[
        elt_handler: def[index: Int](
            var element: Ts[index]
        ) capturing
    ](deinit self):
        # Indexed transfer is legal only through this compiler-private storage
        # field.  A user-facing `tuple[index]^` remains rejected.
        comptime for i in range(len(Ts)):
            elt_handler[i](self.storage[i]^)

    # Current Mojo's family spelling for the same consuming teardown.
    def deinit_with[
        elt_handler: def[index: Int](
            var element: Ts[index]
        ) capturing
    ](deinit self):
        comptime for i in range(len(Ts)):
            elt_handler[i](self.storage[i]^)
