# Upstream's `std/builtin/variadics.mojo` surface for the proof subset:
# `_MLIR`'s raw parameter list type, `TypeList`, the compile-time list a
# variadic type pack (`*Ts: Movable`) is, `ParameterList`, the zero-sized
# runtime value a homogeneous value pack (`*values: Int`) is when it is read
# at run time, and its iterator, and `VariadicList`, the run-time
# homogeneous argument collector, and its iterator.
#
# A pack's `Ts.length` and `Ts[i]` read `TypeList`'s `length` and
# `__getitem_param__`. Its members stand in for upstream's
# `#kgen.param_list.size` and `#kgen.param_list.get` attributes: `len` and a
# subscript of the struct's own raw list, admitted in this module alone.
# Omitted: `_mlir_type` (a member applying `_MLIR.KGENParamListType` is not
# yet an associated comptime expression here), `_raw_length` and
# `_get_type_at_index` (compile-time `__mlir_type.index` values are outside
# the subset, R1531),
# the `@implicit` upcast constructor (`#kgen.upcast`,
# `#kgen.is_refined_type`), and the list vocabulary (`tabulate`, `splat`,
# `_concat`, `reduce`, `any`, `all`, `all_conforms_to`, `contains`, `map`,
# `filter_idx`, `reverse`, `slice`), which the compiler still answers by
# spelling. `TypeList.of` and `ParameterList.of`/`empty_of` are upstream's
# aliases.
#
# `ParameterList`'s `values` is upstream's one raw `KGENParamListType[type]`
# parameter, and a pack's `vs.values` is that list.
#
# `VariadicList`, the run-time homogeneous collector a `*xs: T` parameter is
# in its body, and its `_VariadicListIter` are upstream's, except:
# - the call builds the element pointers' `!pop.array` in its
#   `__passed_varargs__` slot, as upstream's does, but then builds the
#   `Span` over it and the list over the span itself (`struct.create`s),
#   standing for upstream's `@implicit` constructor from that array, so
#   neither that constructor nor `_MLIR.POPArrayType`, its one use, is
#   declared;
# - `__getitem__` returns `ref[origin_of(self)]` rather than upstream's union
#   of the elements' and the list's origins, and binds the span's pointer
#   before indexing it;
# - `__deinit__` reaches each element pointer through the span where
#   upstream's takes `Pointer(to=self[i])`;
# - `_VariadicListIter` writes its constructor out (no `@fieldwise_init`
#   beside it), its `variadic_list_type` names no `origin` (an alias's
#   keyword argument for an infer-only parameter does not resolve yet), and
#   `__iter__` takes `ref self` and names the iterator's origin arguments,
#   which a positional origin argument before a value one cannot be yet;
# - `consume_elements` and `_annihilate` are not declared yet.
# `get_span` addresses the elements through the compiler-private
# `__param_list_address[values]()`, which stands for upstream's
# `global_constant` over `#pop.variadic_to_array`. `ParameterList`'s and
# `VariadicList`'s `write_repr_to` write the text upstream's `FormatStruct`
# builder writes, directly.
from std.builtin.reversed import reversed
from std.collections.optional import Optional
from std.iter import Iterable, Iterator, StopIteration
from std.range import range
from std.reflection.type_info import _unqualified_type_name
from std.span import Span


struct _MLIR:
    comptime KGENTypeType = __mlir_type.`!kgen.type`

    comptime KGENParamListType[elt_type: Self.KGENTypeType] = __mlir_type[
        `!kgen.param_list<`, elt_type, `>`
    ]


struct TypeList[
    Trait: type_of(AnyType), //, values: _MLIR.KGENParamListType[Trait]
](Sized, TrivialRegisterPassable):
    """A compile-time list of types conforming to a common trait.

    `TypeList` provides type-level operations on variadic sequences of types,
    such as reversing, slicing, mapping, and membership testing.

    Parameters:
        Trait: The trait that all types in the list must conform to.
        values: The types in the list.
    """

    comptime length: Int = len(Self.values)
    """The number of types in the list."""

    # Upstream's index type is `SIMDLength`, which a parameter list does not
    # take as an index yet (R1540).
    comptime __getitem_param__[idx: Int] = Self.values[idx]
    """Gets a type at the given index.

    Parameters:
        idx: The index of the type to access.
    """

    @inline(.always)
    def __init__(out self):
        """Constructs a TypeList."""
        pass

    @inline(.always)
    def __len__(self) -> Int:
        """Gets the number of elements in the TypeList.

        Returns:
            The number of elements on the TypeList.
        """
        return Self.length

    comptime of[Trait: type_of(AnyType), //, *Ts: Trait] = TypeList[
        Trait=Trait, Ts.values
    ]
    """Form a compile-time list of types with some elements, uninstantiated.

    Parameters:
        Trait: The type of the elements in the list.
        Ts: The types in the list.
    """


@fieldwise_init
struct _ParameterListIter[type: Copyable, //, *values: type](
    ImplicitlyCopyable, Iterable, Iterator, TrivialRegisterPassable
):
    """Const Iterator for ParameterList.

    Parameters:
        type: The type of the elements in the list.
        values: The values in the list.
    """

    comptime Element = Self.type
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ]: Iterator = Self

    var index: Int

    @inline(.always)
    def __next__(
        mut self,
    ) raises StopIteration -> ref[ImmStaticOrigin] Self.type:
        var index = self.index

        if index >= Self.values.size:
            raise StopIteration()
        self.index = index + 1
        return Self.values[index]

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    @inline(.always)
    def bounds(self) -> Tuple[Int, Optional[Int]]:
        var len = Self.values.size - self.index
        return (len, {len})


struct ParameterList[type: AnyType, //, values: _MLIR.KGENParamListType[type]](
    Sized,
    TrivialRegisterPassable,
    Writable where conforms_to(type, Writable),
):
    """A utility class to access homogeneous variadic parameters.

    `ParameterList` is used by homogenous variadic parameter lists. Unlike
    `VariadicPack` (which is heterogeneous), `ParameterList` requires all
    elements to have the same type.

    Parameters:
        type: The type of the elements in the list.
        values: The values in the list.
    """

    # Upstream's `#kgen.param_list.size` over its raw list: `len` of the
    # struct's own `Self.values` is that attribute here.
    comptime size: Int = len(Self.values)
    """The number of elements in the list."""

    # ===-------------------------------------------------------------------===#
    # Accessors
    # ===-------------------------------------------------------------------===#

    @inline(.always)
    def __len__(self) -> Int:
        """Gets the size of the list.

        Returns:
            The number of elements on the variadic list.
        """
        return Self.size

    @staticmethod
    def get_span() -> Span[Self.type, ImmStaticOrigin]:
        """Gets a span of the elements on the variadic list.

        Returns:
            A span of the elements on the variadic list.
        """
        var first_elt = __param_list_address[Self.values]()
        return Span(unsafe_ptr=first_elt, length=Self.size)

    @inline(.always)
    def __getitem__(self, idx: Int) -> ref[ImmStaticOrigin] Self.type:
        """Gets a single element on the variadic list.

        Args:
            idx: The index of the element to access on the list.

        Returns:
            The element on the list corresponding to the given index.
        """
        return self.get_span()[idx]

    # ===-------------------------------------------------------------------===#
    # Constructors
    # ===-------------------------------------------------------------------===#

    @inline(.always)
    def __init__(out self):
        """Constructs a ParameterList."""
        pass

    comptime empty_of[type: AnyType] = Self.of[type=type]
    """Form an empty compile-time list of values some element type.

    Parameters:
        type: The type of the elements in the list.
    """

    comptime of[type: AnyType, //, *values: type] = ParameterList[
        type=type, values.values
    ]
    """Form a compile-time list of values with some elements, uninstantiated.

    Parameters:
        type: The type of the elements in the list.
        values: The values in the list.
    """

    # ===-------------------------------------------------------------------===#
    # Other
    # ===-------------------------------------------------------------------===#

    def _write_elements[
        is_repr: Bool = False
    ](self, mut writer: Some[Writer]) where conforms_to(Self.type, Writable):
        writer.write_string("(")
        for i in range(len(self)):
            if i > 0:
                writer.write_string(", ")

            comptime if is_repr:
                self[i].write_repr_to(writer)
            else:
                self[i].write_to(writer)
        writer.write_string(")")

    @inline(.never)
    def write_to(
        self, mut writer: Some[Writer]
    ) where conforms_to(Self.type, Writable):
        """Writes the elements of this variadic list to a writer.

        Constraints:
            `type` must conform to `Writable`.

        Args:
            writer: The object to write to.
        """
        self._write_elements(writer)

    @inline(.never)
    def write_repr_to(
        self, mut writer: Some[Writer]
    ) where conforms_to(Self.type, Writable):
        """Writes the repr of this variadic list to a writer.

        Constraints:
            `type` must conform to `Writable`.

        Args:
            writer: The object to write to.
        """
        # Upstream's text: `FormatStruct(writer, "ParameterList").params(
        # TypeNames[Self.type]()).fields(write_fields)`.
        writer.write(
            "ParameterList[", _unqualified_type_name[Self.type](), "]"
        )
        writer.write_string("(")
        self._write_elements[is_repr=True](writer)
        writer.write_string(")")

    # We can only support iteration when the elements are Copyable, because
    # iterators currently need to return the elements by value.
    # Upstream launders the list through `rebind[_MLIR.KGENParamListType[
    # downcast[Self.type, Copyable]]](Self.values)` and returns `{0}`.
    @inline(.always)
    def __iter__(
        ref self,
    ) -> _ParameterListIter[*ParameterList[Self.values]()] where conforms_to(
        Self.type, Copyable
    ):
        """Iterate over the list.

        Returns:
            An iterator to the start of the list.
        """
        return _ParameterListIter[*ParameterList[Self.values]()](0)


# ===-----------------------------------------------------------------------===#
# VariadicList
# ===-----------------------------------------------------------------------===#


struct _VariadicListIter[
    elt_is_mutable: Bool,
    //,
    elt_type: AnyType,
    elt_origin: Origin[mut=elt_is_mutable],
    list_origin: ImmOrigin,
    is_owned: Bool,
](RegisterPassable):
    """Iterator for VariadicList.

    Parameters:
        elt_is_mutable: Whether the elements in the list are mutable.
        elt_type: The type of the elements in the list.
        elt_origin: The origin of the elements.
        list_origin: The origin of the VariadicList.
        is_owned: Whether the elements are owned by the list because they are
                  passed as an 'var' argument.
    """

    comptime variadic_list_type = VariadicList[Self.elt_type, Self.is_owned]

    comptime Element = Self.elt_type

    var index: Int
    var src: Pointer[Self.variadic_list_type, Self.list_origin]

    def __init__(
        out self,
        index: Int,
        ref[Self.list_origin] list: Self.variadic_list_type,
    ):
        self.index = index
        self.src = Pointer(to=list)

    @inline(.always)
    def __next__(
        mut self,
    ) raises StopIteration -> ref[self.src[][0]] Self.elt_type:
        var index = self.index
        if index == len(self.src[]):
            raise StopIteration()
        self.index = index + 1
        return self.src[][index]


struct VariadicList[
    elt_is_mutable: Bool,
    origin: Origin[mut=elt_is_mutable],
    //,
    element_type: AnyType,
    is_owned: Bool,
](
    RegisterPassable,
    Sized,
    Writable where conforms_to(element_type, Writable),
):
    """A utility class to access variadic function arguments of memory-only
    types that may have ownership. It exposes references to the elements in a
    way that can be enumerated.  Each element may be accessed with `elt[]`.

    Parameters:
        elt_is_mutable: True if the elements of the list are mutable for an
                        mut or owned argument.
        origin: The origin of the underlying elements.
        element_type: The type of the elements in the list.
        is_owned: Whether the elements are owned by the list because they are
                  passed as an 'var' argument.
    """

    comptime _EltPointerType = Pointer[Self.element_type, Self.origin]
    # FIXME: This should be the origin of the container, not UntrackedOrigin.
    var _value: Span[Self._EltPointerType, ImmUntrackedOrigin]

    # ===-------------------------------------------------------------------===#
    # Life cycle methods
    # ===-------------------------------------------------------------------===#

    # The destructor for this type is trivial if not an "owned" list.
    comptime __del__is_trivial: Bool = not Self.is_owned

    @inline(.always)
    def __deinit__(deinit self):
        """Destructor that releases elements if owned."""

        # Destroy each element if this variadic has owned elements, destroy
        # them.  We destroy in backwards order to match how arguments are
        # normally torn down when CheckLifetimes is left to its own devices.
        comptime if Self.is_owned:
            comptime assert conforms_to(Self.element_type, Deinitable)

            for i in reversed(range(len(self))):
                # Safety: We own the elements in this list.
                self._value.unsafe_ptr()[unsafe_offset=i].unsafe_mut_cast[
                    True
                ]().unsafe_deinit_pointee()

    # ===-------------------------------------------------------------------===#
    # Trait implementations
    # ===-------------------------------------------------------------------===#

    @inline(.always)
    def __len__(self) -> Int:
        """Gets the size of the list.

        Returns:
            The number of elements on the variadic list.
        """
        return len(self._value)

    # ===-------------------------------------------------------------------===#
    # Operator dunders
    # ===-------------------------------------------------------------------===#

    @inline(.always)
    def __getitem__(ref self, idx: Int) -> ref[origin_of(self)] Self.element_type:
        """Gets a single element on the variadic list.

        Args:
            idx: The index of the element to access on the list.

        Returns:
            A low-level pointer to the element on the list corresponding to the
            given index.
        """
        var elements = self._value.unsafe_ptr()
        return elements[unsafe_offset=idx][]

    def _write_elements[
        is_repr: Bool = False
    ](self, mut writer: Some[Writer]) where conforms_to(
        Self.element_type, Writable
    ):
        writer.write_string("(")
        for i in range(len(self)):
            if i > 0:
                writer.write_string(", ")

            comptime if is_repr:
                self[i].write_repr_to(writer)
            else:
                self[i].write_to(writer)
        writer.write_string(")")

    @inline(.never)
    def write_to(
        self, mut writer: Some[Writer]
    ) where conforms_to(Self.element_type, Writable):
        """Writes the elements of this variadic list to a writer.

        Constraints:
            `element_type` must conform to `Writable`.

        Args:
            writer: The object to write to.
        """
        self._write_elements(writer)

    @inline(.never)
    def write_repr_to(
        self, mut writer: Some[Writer]
    ) where conforms_to(Self.element_type, Writable):
        """Writes the repr of this variadic list to a writer.

        Constraints:
            `element_type` must conform to `Writable`.

        Args:
            writer: The object to write to.
        """
        # Upstream's text: `FormatStruct(writer, "VariadicList").params(
        # TypeNames[Self.element_type]()).fields(write_fields)`.
        writer.write(
            "VariadicList[", _unqualified_type_name[Self.element_type](), "]"
        )
        writer.write_string("(")
        self._write_elements[is_repr=True](writer)
        writer.write_string(")")

    def __iter__(
        ref self,
    ) -> _VariadicListIter[
        Self.element_type, Self.origin, origin_of(self), Self.is_owned
    ]:
        """Iterate over the list.

        Returns:
            An iterator to the start of the list.
        """
        return _VariadicListIter[
            Self.element_type,
            elt_origin = Self.origin,
            list_origin = origin_of(self),
            is_owned = Self.is_owned,
        ](0, self)
