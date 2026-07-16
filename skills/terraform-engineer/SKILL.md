---
name: terraform-engineer
description: Terraform infrastructure as code across AWS, Azure, or GCP. Use for module development, state management, provider configuration, multi-environment workflows, infrastructure testing.
---

# Terraform Engineer

## Core Workflow

1. **Analyze infrastructure** - Review requirements, existing code, cloud platforms
2. **Design modules** - Create composable, validated modules with clear interfaces
3. **Implement state** - Configure remote backends with locking and encryption
4. **Secure infrastructure** - Apply security policies, least privilege, encryption
5. **Test and validate** - Run terraform plan, policy checks, automated tests

## Reference Guide

Load detailed guidance based on context:

| Topic          | Reference                        | Load When                                        |
| -------------- | -------------------------------- | ------------------------------------------------ |
| Modules        | `references/module-patterns.md`  | Creating modules, inputs/outputs, versioning     |
| State          | `references/state-management.md` | Remote backends, locking, workspaces, migrations |
| Providers      | `references/providers.md`        | AWS/Azure/GCP configuration, authentication      |
| Testing        | `references/testing.md`          | terraform plan, terratest, policy as code        |
| Best Practices | `references/best-practices.md`   | DRY patterns, naming, security, cost tracking    |

## Constraints

- Use semantic versioning for modules
- Enable remote state with locking
- Validate inputs with validation blocks
- Use consistent naming conventions
- Tag all resources for cost tracking
- Document module interfaces
- Pin provider versions
- Run terraform fmt and validate
- Reference secrets via sensitive variables or a secrets manager
- Parameterize environment-specific values through tfvars
- Keep module dependency graphs acyclic
- Add `.terraform/` to .gitignore

## Output Templates

When implementing Terraform solutions, provide:

1. Module structure (main.tf, variables.tf, outputs.tf)
2. Backend configuration for state
3. Provider configuration with versions
4. Example usage with tfvars
5. Brief explanation of design decisions
