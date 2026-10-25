# A `None` slice bound or stride is an omitted one: the literal passes it to
# its constructor's `Optional[Int]` or `NoneType` parameter, so a `None`
# stride selects `ContiguousSlice`, as at the pin.
def main():
    var xs: List[Int] = [0, 1, 2, 3, 4]
    print(xs[1:5:None])
    print(xs[None:3])
    print(xs[None:None:2])
