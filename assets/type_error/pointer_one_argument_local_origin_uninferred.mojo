# A type position must bind `Pointer`'s `origin`, which has no default.
# expect: failed to infer parameter 'origin'
def main():
    var x = 5
    var p = Pointer(to=x)
    var y: Pointer[Int] = p
    print(y[])
