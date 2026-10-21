# expect: invalidated interior reference
# Growing the list before a `break` out of a `try` carries the invalidation
# to the loop's exit, where the view is used.
# requires: stdlib
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var v = ys[0].rstrip()
    for i in range(3):
        try:
            if i == 1:
                ys.append(String("zz"))
                break
        except:
            pass
    print(String(v))
