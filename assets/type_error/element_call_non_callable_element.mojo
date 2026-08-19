# expect: is not callable
# The bare element-call spelling dispatches the subscripted element, so a
# non-callable element type is rejected as such (not as a parameter-application
# parse), naming the element rather than the container.
def main():
    var xs: List[Int] = [1, 2, 3]
    print(xs[0](3))
