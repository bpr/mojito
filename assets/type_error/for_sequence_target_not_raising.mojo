# expect: a sequence destructuring assignment requires a surrounding 'try'
# A sequence loop target checks each length with a raising call, so it needs a raising context.
def main():
    var rows: List[List[Int]] = [[1, 2, 3], [4]]
    for [a, *_] in rows:
        print(a)
