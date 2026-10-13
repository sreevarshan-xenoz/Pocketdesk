name: Contributor Welcome
description: Open a discussion about contributing to Pocketdesk
labels: help wanted
body:
  - type: markdown
    attributes:
      value: |
        Thanks for your interest in contributing to Pocketdesk!
  - type: textarea
    id: interest
    attributes:
      label: Your Interests
      placeholder: |
        What area are you interested in contributing to?
        - Android client (Flutter + Rust FFI)
        - Wayland/screen capture
        - KDE Connect plugins
        - Performance optimization
        - Documentation
        - Testing
  - type: textarea
    id: experience
    attributes:
      label: Your Experience
      placeholder: Brief description of your relevant experience (optional)
  - type: textarea
    id: time
    attributes:
      label: Availability
      placeholder: How much time are you looking to contribute?
