# A display of a capturing lambda cannot be reassigned from another display:
# each lambda is its own closure storage type, so the second display's
# element is not the first's.
# expect: type mismatch for collection display element
def main():
    var k = 3
    var fns = [lambda (x: Int) {k} -> Int: x * k]
    fns = [lambda (x: Int) {k} -> Int: x + k]
    print(fns[0](2))
