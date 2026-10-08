# Upstream's `std/builtin/variadics.mojo` surface for the proof subset:
# `ParameterList`, the zero-sized runtime value a homogeneous value pack
# (`*values: Int`) is when it is read at run time, and its iterator.
#
# Upstream's `values` is one `KGENParamListType[type]` parameter; here it is
# the variadic `*values: type` upstream's own `_ParameterListIter` spells.
# `get_span` addresses the elements through the compiler-private
# `__param_list_address[*values]()`, which stands for upstream's
# `global_constant` over `#pop.variadic_to_array`.
from std.collections.optional import Optional
from std.iter import Iterable, Iterator, StopIteration
from std.span import Span


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

    def __next__(
        mut self,
    ) raises StopIteration -> ref[ImmStaticOrigin] Self.type:
        var index = self.index

        if index >= len(Self.values):
            raise StopIteration()
        self.index = index + 1
        return Self.values[index]

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    def bounds(self) -> Tuple[Int, Optional[Int]]:
        var len = len(Self.values) - self.index
        return (len, {len})


struct ParameterList[type: AnyType, //, *values: type](
    Sized, TrivialRegisterPassable
):
    """A utility class to access homogeneous variadic parameters.

    `ParameterList` is used by homogenous variadic parameter lists. Unlike
    `VariadicPack` (which is heterogeneous), `ParameterList` requires all
    elements to have the same type.

    Parameters:
        type: The type of the elements in the list.
        values: The values in the list.
    """

    # ===-------------------------------------------------------------------===#
    # Accessors
    # ===-------------------------------------------------------------------===#

    def __len__(self) -> Int:
        """Gets the size of the list.

        Returns:
            The number of elements on the variadic list.
        """
        return len(Self.values)

    @staticmethod
    def get_span() -> Span[Self.type, ImmStaticOrigin]:
        """Gets a span of the elements on the variadic list.

        Returns:
            A span of the elements on the variadic list.
        """
        var first_elt = __param_list_address[*Self.values]()
        return Span(unsafe_ptr=first_elt, length=len(Self.values))

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

    def __init__(out self):
        """Constructs a ParameterList."""
        pass

    # We can only support iteration when the elements are Copyable, because
    # iterators currently need to return the elements by value.
    def __iter__(
        ref self,
    ) -> _ParameterListIter[*Self.values] where conforms_to(Self.type, Copyable):
        """Iterate over the list.

        Returns:
            An iterator to the start of the list.
        """
        return _ParameterListIter[*Self.values](0)
