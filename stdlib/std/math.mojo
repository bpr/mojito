# Upstream's `std/math/math.mojo`, holding its operation traits and the
# rounding helpers `floor`/`ceil`/`trunc`/`ceildiv`. The prelude exports
# `Absable`, `Powable`, and `Roundable`, as upstream's does; `abs`/`round`/
# `divmod` are still compiler builtins (R1458).
#
# Each helper is generic over the trait that supplies its dunder. The
# compiler-modelled `Int`, `UInt`, and `Float64` conform as upstream's `SIMD`
# declares, other `SIMD` values only in part (R1457), and each answers a
# requirement intrinsically.


trait Ceilable:
    def __ceil__(self) -> Self:
        ...


trait Floorable:
    def __floor__(self) -> Self:
        ...


trait CeilDivable:
    def __ceildiv__(self, denominator: Self) -> Self:
        ...


trait CeilDivableRaising:
    def __ceildiv__(self, denominator: Self) raises -> Self:
        ...


trait Truncable:
    def __trunc__(self) -> Self:
        ...


trait Absable:
    def __abs__(self) -> Self:
        ...


# Upstream's refines `ImplicitlyCopyable`, which a declared trait cannot
# refine yet (R1459).
trait DivModable:
    def __divmod__(self, denominator: Self) -> Tuple[Self, Self]:
        ...


trait Powable:
    def __pow__(self, exp: Self) -> Self:
        ...


trait Roundable:
    def __round__(self) -> Self:
        ...

    def __round__(self, ndigits: Int) -> Self:
        ...


@inline(.always)
def floor[T: Floorable](value: T) -> T:
    return value.__floor__()


@inline(.always)
def ceil[T: Ceilable](value: T) -> T:
    return value.__ceil__()


@inline(.always)
def trunc[T: Truncable](value: T) -> T:
    return value.__trunc__()


@inline(.always)
def ceildiv[T: CeilDivable](numerator: T, denominator: T) -> T:
    return numerator.__ceildiv__(denominator)
