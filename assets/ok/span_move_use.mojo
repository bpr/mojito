# Moving a live Span is ordinary aggregate transfer: the moved-to binding
# carries the view's loans and keeps reading the List's storage.
def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    var sp = Span(xs)
    var sp2 = sp^
    print(sp2[1])
