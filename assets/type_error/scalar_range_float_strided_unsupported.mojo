# expect: float strided ranges are not supported
# Upstream's fma-driven float strided range is a recorded subset gap; the
# rejection is explicit rather than a misleading overload mismatch.
def main():
    for x in range(Float32(0.0), Float32(1.0), Float32(0.25)):
        print(x)
