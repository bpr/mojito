# expect: invalidated interior reference
# Growing the list before a `continue` out of a `try` carries the
# invalidation to the next iteration, where the view is used.
# requires: stdlib
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    for i in range(3):
        print(String(v))
        try:
            if i == 1:
                ys.append(String("zz"))
                continue
        except:
            pass
