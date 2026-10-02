# expect: invalidated interior reference
# A view of a List element's owned interior names that interior
# (`ys["element"]["bytes"]`), so growing the list while the view is live
# invalidates it.
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    ys.append(String("zz"))
    print(String(v))
