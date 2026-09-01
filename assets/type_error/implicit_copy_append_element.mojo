# expect: cannot be implicitly copied
# `append(var value)` cannot implicitly copy a nested `List` place.
def main():
    var rows = List[List[Int]]()
    var row: List[Int] = [1]
    rows.append(row)
    print(len(rows))
