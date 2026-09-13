# `input(prompt)` writes the prompt and reads one line, raising `EOF` at end
# of input (the `input-eof` conformance case), so the read goes through a
# helper that catches it and a noninteractive run still finishes.
def read_line(prompt: String) -> String:
    try:
        return input(prompt)
    except e:
        return String("")

def my_function(text: String):
    var name = read_line(String("Enter your name: "))
    print(text + name)

def main(): my_function(String("Hello, "))
