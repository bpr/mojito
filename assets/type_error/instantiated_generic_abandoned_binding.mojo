# Instantiating a generic does not excuse its parametric body: `y` is typed by
# a parameter whose bounds do not prove `Deinitable`, so the body abandons it
# whatever `outer[Int](7)` supplies. The abstract template is kept alongside
# its specializations for exactly this check.
# expect: 'y' abandoned without being explicitly destroyed
def pick[T: ImplicitlyCopyable & Writable](x: T) -> T:
    return x

def outer[T: ImplicitlyCopyable & Writable](x: T):
    var y = pick(x)
    print(y)

def main():
    outer[Int](7)
