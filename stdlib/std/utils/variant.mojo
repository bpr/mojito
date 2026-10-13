# Self-hosted `Variant`: current Mojo's tagged union, written over the
# compiler-private `__VariantStorage[*Self.Ts]` storage — the intrinsic tag+payload
# union the register VM executes and the native layout engine lays out
# (upstream keeps its storage in an MLIR `!kgen.variant`). The public API and
# the pack-driven protocol bodies follow upstream's `utils/variant.mojo`.
#
# The template serves every instance. Each storage operation selects the
# alternative `_get_type_index[T, *Self.Ts]()` names, which the elaborator
# closes per instance; a `T` the pack lacks fails the instantiation with
# upstream's "Type does not exist in Variant.". The
# `comptime if Self.Ts.contains[T](): pass` lines stand where upstream calls
# `Self._check[T]()`, whose `comptime assert` Mojito does not parse yet.
from std.reflection.type_info import _unqualified_type_name

struct Variant[*Ts: AnyType](
    Copyable where Ts.all_conforms_to[Copyable](),
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Equatable where Ts.all_conforms_to[Equatable](),
    Hashable where Ts.all_conforms_to[Hashable](),
    ImplicitlyCopyable where Ts.all_conforms_to[ImplicitlyCopyable](),
    Movable where Ts.all_conforms_to[Movable](),
    Writable where Ts.all_conforms_to[Writable](),
):
    var _storage: __VariantStorage[*Self.Ts]

    @implicit
    def __init__[T: Movable](out self, var value: T):
        comptime if Self.Ts.contains[T]():
            pass
        self._storage = __VariantStorage[*Self.Ts](value^)

    def __init__[T: AnyType, //, F: def() -> T](out self, *, init_with: F):
        comptime if Self.Ts.contains[T]():
            pass
        self._storage = __VariantStorage[*Self.Ts](init_with=init_with)

    @__unsafe_nested_origins_read_only
    def __getitem_param__[T: AnyType](ref self) -> ref[origin_of(self)] T:
        comptime if Self.Ts.contains[T]():
            pass
        return self._storage[T]

    def __eq__(self, other: Self) -> Bool where Self.Ts.all_conforms_to[Equatable]():
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            if self._storage.isa[T]():
                if not other._storage.isa[T]():
                    return False
                return self._storage.unsafe_get[T]() == other._storage.unsafe_get[T]()
        return False

    def __ne__(self, other: Self) -> Bool where Self.Ts.all_conforms_to[Equatable]():
        return not (self == other)

    def __hash__[H: Hasher](self, mut hasher: H) where Self.Ts.all_conforms_to[Hashable]():
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            if self._storage.isa[T]():
                hasher.update(UInt8(i))
                hasher.update(self._storage.unsafe_get[T]())
                return

    def write_to(self, mut writer: Some[Writer]) where Self.Ts.all_conforms_to[Writable]():
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            if self._storage.isa[T]():
                writer.write(self._storage.unsafe_get[T]())
                return

    def write_repr_to(self, mut writer: Some[Writer]) where Self.Ts.all_conforms_to[Writable]():
        writer.write("Variant[")
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            comptime if i > 0:
                writer.write(", ")
            writer.write(_unqualified_type_name[T]())
        writer.write("](")
        comptime for i in range(Self.Ts.length):
            comptime T = Self.Ts[i]
            if self._storage.isa[T]():
                writer.write(repr(self._storage.unsafe_get[T]()))
        writer.write(")")

    def unwrap[T: Movable](deinit self) -> T:
        comptime if Self.Ts.contains[T]():
            pass
        return self._storage.unwrap[T]()

    def unsafe_unwrap[T: Movable](deinit self) -> T:
        comptime if Self.Ts.contains[T]():
            pass
        return self._storage.unsafe_unwrap[T]()

    def replace[Tin: Movable & Deinitable, Tout: Movable](mut self, var value: Tin) -> Tout:
        comptime if Self.Ts.contains[Tin]():
            pass
        return self._storage.replace[Tin, Tout](value^)

    def unsafe_replace[Tin: Movable, Tout: Movable](mut self, var value: Tin) -> Tout:
        comptime if Self.Ts.contains[Tin]():
            pass
        return self._storage.unsafe_replace[Tin, Tout](value^)

    def set[T: Movable](mut self, var value: T) where Self.Ts.all_conforms_to[Deinitable]():
        comptime if Self.Ts.contains[T]():
            pass
        self._storage.set[T](value^)

    def set[T: AnyType, //, F: def() -> T](
        mut self, *, init_with: F
    ) where Self.Ts.all_conforms_to[Deinitable]():
        comptime if Self.Ts.contains[T]():
            pass
        self._storage.set(init_with=init_with)

    def isa[T: AnyType](self) -> Bool:
        comptime if Self.Ts.contains[T]():
            pass
        return self._storage.isa[T]()

    def unsafe_get[T: AnyType](ref self) -> ref[origin_of(self)] T:
        comptime if Self.Ts.contains[T]():
            pass
        return self._storage[T]

    @staticmethod
    def is_type_supported[T: Movable]() -> Bool:
        return Self.Ts.contains[T]()

    def deinit_with[T: AnyType, F: def(var T)](deinit self, deinit_func: F, /):
        comptime if Self.Ts.contains[T]():
            pass
        self._storage.deinit_with(deinit_func)
