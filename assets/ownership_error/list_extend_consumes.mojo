# expect: use of 'b' after it was transferred
# extend consumes its argument on both compilers: reading the source list
# afterward rejects.
def main():
    var a: List[Int] = [1, 2]
    var b: List[Int] = [3, 4]
    a.extend(b^)
    print(len(b))
