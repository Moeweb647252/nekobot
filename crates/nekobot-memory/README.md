# Nekobot memory middleware

This crate provides a middleware for Nekobot that allows agents to store and retrieve information in a memory system. The memory system is implemented via turso's vector feature.

## Features

- Remembering: Agents can store information in the memory system, allowing them to recall it later. This is useful for maintaining context across sessions and enabling more complex behaviors.
- Retrieval: Agents can query the memory system to retrieve relevant information based on a given query. This allows agents to access past interactions and use that information to inform their responses and actions.
- Forgetting: Agents can also choose to forget certain information, allowing them to manage their memory and ensure that they only retain relevant and useful information.
