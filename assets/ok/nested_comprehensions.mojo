def main():
    var matrix = [
        [1, 2, 3],
        [4, 5, 6],
        [7, 8, 9]
    ]
    var flat = [cell * 2 for row in matrix for cell in row]
    for f in flat:
        print(f)
