# expect: a sequence destructuring assignment requires a surrounding 'try'
# A sequence destructuring raises on a wrong length, so it needs a raising context.
def main():
    var values: List[Int] = [1, 2]
    var [a, b] = values
    print(a, b)
