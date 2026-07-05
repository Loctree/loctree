struct Bridge {
    let name: String

    func run(input: StreamInput) {
        input.finish()
    }
}

struct StreamInput {
    func finish() {}
}
