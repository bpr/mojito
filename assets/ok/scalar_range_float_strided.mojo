# A floating-point strided range iterates by index: element `k` is the fused
# multiply-add `k * step + start`, and the range has
# `ceil((end - start) / step)` elements (none for a zero step).
def main():
    for x in range(Float64(0.1), Float64(1.0), Float64(0.1)):
        print(x)
    for y in range(Float64(1.0), Float64(0.0), Float64(-0.3)):
        print(y)
    for z in range(Float32(0.5), Float32(2.0), Float32(0.5)):
        print(z)
    for w in range(Float64(0.0), Float64(1.0), Float64(0.0)):
        print(w)
