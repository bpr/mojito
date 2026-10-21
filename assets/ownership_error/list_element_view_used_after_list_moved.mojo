# expect: use of uninitialized value 'ys'
# A view of a List element names the list as its origin, so using the view
# after the list moves away is a use of the uninitialized list.
# requires: stdlib
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    var zs = ys^
    print(String(v))
    print(len(zs))
