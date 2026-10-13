name: Feature Request
description: Suggest a new feature or improvement
labels: enhancement
body:
  - type: markdown
    attributes:
      value: |
        Have an idea for Pocketdesk? We'd love to hear it!
  - type: textarea
    id: feature
    attributes:
      label: Feature Description
      placeholder: Describe the feature you'd like to see
    validations:
      required: true
  - type: textarea
    id: motivation
    attributes:
      label: Motivation
      description: Why would this feature be useful?
      placeholder: Explain the use case
    validations:
      required: true
  - type: textarea
    id: alternatives
    attributes:
      label: Alternatives Considered
      placeholder: Any alternative approaches you've considered
  - type: textarea
    id: additional
    attributes:
      label: Additional Context
      placeholder: Any other relevant information
