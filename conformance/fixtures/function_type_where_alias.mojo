# The changelog's `comptime Kernel = def[w: Int](Int) thin -> None where
# (...)` alias spelling runs on both (prints 7), as the inline-bound
# spelling of the same contract does
# (assets/ok/function_type_where_bound.mojo).
comptime Kernel = def[w: Int](Int) thin -> None where (w > 0, "width must be positive")

def kernel[w: Int](x: Int) where (w > 0, "width must be positive"):
    print(w + x)

def apply[F: Kernel](x: Int):
    F[4](x)

def main():
    apply[kernel](3)
