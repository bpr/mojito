# Upstream's `std/math/__init__.mojo`, re-exporting the ported names in
# upstream's order. Not yet re-exported, since their declarations are not
# ported: `nextafter` from `std.utils.numerics`, and from `.math` `acos`,
# `acosh`, `align_down`, `align_up`, `mulwide`, `mulhi`, `asin`, `asinh`,
# `atan`, `atan2`, `atanh`, `cbrt`, `cos`, `cosh`, `divmod`, `erf`, `erfc`,
# `exp`, `exp2`, `expm1`, `comb`, `factorial`, `gamma`, `gcd`, `hypot`,
# `isclose`, `rsqrt`, `j0`, `j1`, `lcm`, `lgamma`, `log1p`, `log2`,
# `log10`, `logb`, `perm`, `pow`, `recip`, `remainder`, `round`,
# `scalb`, `sin`, `sinh`, `sqrt`, `tan`, `tanh`, `ulp`, `y0`, and `y1`.
# The `std.utils.numerics` import follows `.math`'s, where upstream's
# leads: `std._plugin` is then still loading when `std.math.math` links,
# so its `CurrentPlugin` alias is not yet bound (R592).
"""Math functions and constants: trig, exponential, logarithmic, and special functions.

The `math` package provides mathematical functions and constants for numerical
computation. It includes standard mathematical operations from trigonometry,
exponential and logarithmic functions, special functions, and numerical
utilities. This package implements both precise mathematical operations and fast
approximations for performance-critical code, along with support for rounding,
clamping, and IEEE 754 floating-point behavior.

Use this package for scientific computing, numerical algorithms, graphics and
game development, statistical calculations, or any application requiring
mathematical operations beyond basic arithmetic. The `fast` module provides
optimized approximations when absolute precision can be traded for performance.
"""

from .constants import e, pi, tau

# These are not part of Python's `math` package, but we define them here.
from .math import (
    Absable,
    Ceilable,
    CeilDivable,
    CeilDivableRaising,
    DivModable,
    Floorable,
    Powable,
    Roundable,
    Truncable,
    abs,
    ceil,
    ceildiv,
    clamp,
    copysign,
    floor,
    fma,
    frexp,
    iota,
    ldexp,
    log,
    max,
    min,
    modf,
    trunc,
)

# In Python, these are in the `math` package, so we also expose them here.
from std.utils.numerics import inf, isfinite, isinf, isnan, nan
