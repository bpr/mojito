# Public heterogeneous tuple, an ordinary variadic struct generator: this one
# template serves every element list.  `__RuntimeTuple` is a compiler-private
# pack aggregate: users see this nominal struct, while the private field gives
# the compiler exact per-index storage. The storage is trivially destructible,
# as upstream's `!kgen.struct`, and `__deinit__` destroys each element through
# `unsafe_deinit_pointee`. The default and copy
# initializers build through `init_with`, and the copy and move initializers
# take upstream's `__copy_ctor_is_trivial`/`__move_ctor_is_trivial` fast paths.

from std.reflection.type_info import _unqualified_type_name
from std.range import range
from std.hash.hasher import Hasher


@explicit_destroy(
    "Use `deinit_with()` to explicitly destroy a `Tuple` with"
    " non-`Deinitable` elements"
)
struct Tuple[*Ts: Movable](
    Comparable where conforms_to(Ts.values, Comparable) and conforms_to(Ts.values, Equatable),
    Copyable where conforms_to(Ts.values, Copyable),
    Defaultable where conforms_to(Ts.values, Defaultable),
    Equatable where conforms_to(Ts.values, Equatable),
    Hashable where conforms_to(Ts.values, Hashable),
    ImplicitlyCopyable where conforms_to(Ts.values, ImplicitlyCopyable),
    Deinitable where conforms_to(Ts.values, Deinitable),
    Movable,
    RegisterPassable where conforms_to(Ts.values, RegisterPassable),
    Sized,
    Writable where conforms_to(Ts.values, Writable),
):
    comptime element_types = Self.Ts

    comptime __del__is_trivial = Self.Ts.all[IsTriviallyDeinitable]()
    comptime __copy_ctor_is_trivial = Self.Ts.all[IsTriviallyCopyable]()
    comptime __move_ctor_is_trivial = Self.Ts.all[IsTriviallyMovable]()

    var storage: __RuntimeTuple[*Self.Ts]

    @inline(.nodebug)
    def __init__(out self, var *args: *Self.Ts):
        self.storage = __RuntimeTuple(*args^)

    # Upstream assigns `__get_address_as_uninit_lvalue(...) = init_with[i]()`,
    # constructing each element in place; Mojito writes it (R2024).
    @inline(.nodebug)
    def __init__[F: def[idx: Int]() -> Self.Ts[idx]](out self, *, init_with: F):
        __mlir_op.`lit.ownership.mark_initialized`(
            __get_mvalue_as_litref(self.storage)
        )
        comptime for i in range(Self.__len__()):
            Pointer(to=self[i]).unsafe_write(init_with[i]())

    # The storage, upstream's `!kgen.struct`, is trivially destructible: the
    # destructor destroys each element itself.
    def __deinit__(
        deinit self,
    ) where Self.Ts.all_conforms_to[Deinitable]():
        comptime for i in range(Self.__len__()):
            Pointer(to=self[i]).unsafe_deinit_pointee()

    @inline(.nodebug)
    def __init__(out self) where conforms_to(Self.Ts.values, Defaultable):
        self = Self(init_with=lambda [i: Int]() -> Self.Ts[i]: Self.Ts[i]())

    # Current Mojo's compile-time-index hook.
    @inline(.nodebug)
    def __getitem_param__[index: Int](
        ref self
    ) -> ref[origin_of(self)] Self.Ts[index]:
        return self.storage[index]

    @inline(.nodebug)
    def __len__(self) -> Int:
        return len(self.storage)

    @always_inline("builtin")
    @staticmethod
    def __len__() -> Int:
        return Self.Ts.length

    @inline(.always)
    def __eq__(self, other: Self) -> Bool where conforms_to(
        Self.Ts.values,Equatable
    ):
        comptime for i in range(len(Self.Ts)):
            if self.storage[i] != other.storage[i]:
                return False
        return True

    def __ne__(self, other: Self) -> Bool where conforms_to(
        Self.Ts.values,Equatable
    ):
        comptime for i in range(len(Self.Ts)):
            if self.storage[i] != other.storage[i]:
                return True
        return False

    @inline(.always)
    def __lt__(self, other: Self) -> Bool where conforms_to(
        Self.Ts.values,Comparable
    ) and conforms_to(Self.Ts.values, Equatable):
        comptime for i in range(len(Self.Ts)):
            if self.storage[i] < other.storage[i]:
                return True
            if other.storage[i] < self.storage[i]:
                return False
        return False

    @inline(.always)
    def __le__(self, other: Self) -> Bool where conforms_to(
        Self.Ts.values,Comparable
    ) and conforms_to(Self.Ts.values, Equatable):
        comptime for i in range(len(Self.Ts)):
            if self.storage[i] < other.storage[i]:
                return True
            if other.storage[i] < self.storage[i]:
                return False
        return True

    @inline(.always)
    def __gt__(self, other: Self) -> Bool where conforms_to(
        Self.Ts.values,Comparable
    ) and conforms_to(Self.Ts.values, Equatable):
        comptime for i in range(len(Self.Ts)):
            if other.storage[i] < self.storage[i]:
                return True
            if self.storage[i] < other.storage[i]:
                return False
        return False

    @inline(.always)
    def __ge__(self, other: Self) -> Bool where conforms_to(
        Self.Ts.values,Comparable
    ) and conforms_to(Self.Ts.values, Equatable):
        comptime for i in range(len(Self.Ts)):
            if other.storage[i] < self.storage[i]:
                return True
            if self.storage[i] < other.storage[i]:
                return False
        return True

    def __hash__[H: Hasher](self, mut hasher: H) where conforms_to(
        Self.Ts.values,Hashable
    ):
        comptime for i in range(len(Self.Ts)):
            hasher.update(self.storage[i])

    @inline(.nodebug)
    def __contains__[T: Equatable](self, value: T) -> Bool:
        comptime for i in range(len(Self.Ts)):
            comptime if T == Self.Ts[i]:
                if rebind[T](self[i]) == value:
                    return True
        return False

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.Ts.values,Writable
    ):
        writer.write("(")
        comptime for i in range(len(Self.Ts)):
            comptime if i > 0:
                writer.write(", ")
            writer.write(self.storage[i])
        comptime if len(Self.Ts) == 1:
            writer.write(",")
        writer.write(")")

    # Upstream's `Tuple[<element types>](<element reprs>)`.
    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.Ts.values,Writable
    ):
        writer.write(_unqualified_type_name[Self](), "(")
        comptime for i in range(len(Self.Ts)):
            comptime if i > 0:
                writer.write(", ")
            writer.write(repr(self.storage[i]))
        writer.write(")")

    # Upstream's signatures. Each element is moved out through a pointer, as
    # upstream moves it; the trivially destructible storage ends with `self`.
    @inline(.nodebug)
    def reverse(deinit self, out result: Tuple[*Self.Ts.reverse()]):
        __mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(result))
        comptime for i in range(Self.Ts.length):
            Pointer(to=result[i]).unsafe_write(
                Pointer(to=self[Self.Ts.length - 1 - i]).unsafe_take_pointee()
            )

    @inline(.nodebug)
    def concat[*OtherTs: Movable](
        deinit self,
        deinit other: Tuple[*OtherTs],
        out result: Tuple[*TypeList._concat[Self.Ts.values, OtherTs.values]()],
    ):
        __mlir_op.`lit.ownership.mark_initialized`(__get_mvalue_as_litref(result))
        comptime self_len = Self.Ts.length
        comptime for i in range(self_len):
            Pointer(to=rebind[Self.Ts[i]](result[i])).unsafe_write(
                Pointer(to=self[i]).unsafe_take_pointee()
            )
        comptime for i in range(OtherTs.length):
            Pointer(to=rebind[OtherTs[i]](result[self_len + i])).unsafe_write(
                Pointer(to=other[i]).unsafe_take_pointee()
            )

    @inline(.nodebug)
    def consume_elements[
        F: def[idx: Int](var elt: Self.Ts[idx])
    ](deinit self, elt_handler: F, /):
        comptime for i in range(len(Self.Ts)):
            elt_handler[i](Pointer(to=self[i]).unsafe_take_pointee())

    # Current Mojo's family spelling for the same consuming teardown.
    def deinit_with[
        F: def[idx: Int](var elt: Self.Ts[idx])
    ](deinit self, deinit_func: F, /):
        self^.consume_elements(deinit_func)

    # Upstream's copy initializer. `self.storage = copy.storage.copy()`
    # stands for upstream's `self._mlir_value = copy._mlir_value`: the
    # private storage is not implicitly copyable at a symbolic pack (R2028).
    @inline(.nodebug)
    def __init__(
        out self, *, copy: Self
    ) where Self.Ts.all_conforms_to[Copyable]():
        comptime if Self.__copy_ctor_is_trivial:
            self.storage = copy.storage.copy()
        else:
            self = Self(
                init_with=lambda [i: Int]() -> Self.Ts[i]: copy[i].copy()
            )

    # Upstream's move initializer. The destructor on `move` is disabled by
    # its `deinit` convention, and its storage is trivially destructible.
    @inline(.nodebug)
    def __init__(
        out self, *, deinit move: Self
    ) where Self.Ts.all_conforms_to[Movable]():
        comptime if Self.__move_ctor_is_trivial:
            self.storage = move.storage
        else:
            __mlir_op.`lit.ownership.mark_initialized`(
                __get_mvalue_as_litref(self.storage)
            )
            comptime for i in range(Self.__len__()):
                Pointer(to=self[i]).unsafe_write_move_from(Pointer(to=move[i]))
