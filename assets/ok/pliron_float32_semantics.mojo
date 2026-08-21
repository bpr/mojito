# Float32 scalars: every operation computes at f64 and rounds the result to
# single precision, values print as the f64 view of the rounded number, and
# `/` keeps the Float32 lane (no Float64 promotion).
def main():
    var f = Float32(0.1)
    print(f) # the f64 view: 0.10000000149011612
    print(f + f)
    print(f * f)
    var g = Float32(1.5)
    print(g - Float32(0.25))
    print(g / Float32(0.5)) # stays Float32
    print(-g)
    print(g < Float32(2.0))
    print(g == Float32(1.5))
    print(Float64(f)) # widening is exact
    var h = Float32(16777217.0) # rounds to 16777216 in binary32
    print(h)
    print(Float32(2.75))
    print(Bool(f))
    print(Bool(Float32(0.0)))
